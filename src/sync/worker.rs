//! Long-running polling sync worker for one account/folder pair.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::auth::MailCredential;
use crate::imap::{IdleOutcome, IdleRequest, ImapError, ImapIdle, ImapSync};
use crate::ipc::Hub;
use crate::models::AccountId;

use super::state::{publish_sync_state, SyncState, SyncStateEvent};
use super::{reconcile_folder, SyncError};

/// Thin helper that publishes per-account `Topic::SyncState`
/// transitions for one worker. Does no coalescing — subscribers handle
/// rate-limiting if they care.
struct StateReporter {
    hub: Arc<Hub>,
    account_id: AccountId,
}

impl StateReporter {
    fn new(hub: Arc<Hub>, account_id: AccountId) -> Self {
        Self { hub, account_id }
    }

    async fn transition(&mut self, state: SyncState, last_error: Option<String>) {
        publish_sync_state(
            &self.hub,
            SyncStateEvent::new(self.account_id, state, last_error),
        )
        .await;
    }
}

/// Tunable parameters for a single sync worker's polling + backoff loop.
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    /// Delay between successful poll cycles in non-IDLE mode.
    pub poll_interval: Duration,
    /// Maximum time a single IMAP IDLE wait may take before reconnecting.
    pub idle_timeout: Duration,
    /// Backoff applied after the first failed cycle.
    pub initial_backoff: Duration,
    /// Upper bound on the exponential backoff between failed cycles.
    pub max_backoff: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(29 * 60),
            initial_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// Refreshes [`MailCredential`]s mid-flight so a long-running sync
/// worker keeps a valid OAuth bearer between poll cycles.
#[async_trait::async_trait]
pub trait WorkerCredentialResolver: Send + Sync {
    /// Return a fresh [`MailCredential`] for `account_id`, refreshing
    /// any underlying OAuth token if necessary.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`SyncError::MissingCredentials`] if the account has no stored
    ///   credentials at all.
    /// - [`SyncError::Credential`] if a refresh round-trip fails for a
    ///   reason that is not itself an auth rejection (network, decode,
    ///   secret-store IO).
    /// - [`SyncError::Imap`] wrapping [`crate::imap::ImapError::Auth`] if
    ///   the upstream rejects the refreshed credential.
    async fn resolve(&self, account_id: AccountId) -> Result<MailCredential, SyncError>;
}

#[derive(Clone)]
pub(crate) enum WorkerCredentialSource {
    Static(MailCredential),
    Dynamic {
        account_id: AccountId,
        resolver: Arc<dyn WorkerCredentialResolver>,
    },
}

impl WorkerCredentialSource {
    pub(crate) fn static_credential(credential: MailCredential) -> Self {
        Self::Static(credential)
    }

    pub(crate) fn dynamic(
        account_id: AccountId,
        resolver: Arc<dyn WorkerCredentialResolver>,
    ) -> Self {
        Self::Dynamic {
            account_id,
            resolver,
        }
    }

    async fn resolve(&self) -> Result<MailCredential, SyncError> {
        match self {
            Self::Static(credential) => Ok(credential.clone()),
            Self::Dynamic {
                account_id,
                resolver,
            } => resolver.resolve(*account_id).await,
        }
    }
}

pub(crate) struct SyncWorker {
    pub(crate) pool: SqlitePool,
    pub(crate) hub: Arc<Hub>,
    pub(crate) imap: Arc<dyn ImapSync>,
    pub(crate) idle: Option<Arc<dyn ImapIdle>>,
    pub(crate) account_id: AccountId,
    pub(crate) folder_name: String,
    pub(crate) credential: WorkerCredentialSource,
    pub(crate) cancel: CancellationToken,
    pub(crate) config: WorkerConfig,
}

pub(crate) struct PollingWorker {
    pub(crate) pool: SqlitePool,
    pub(crate) hub: Arc<Hub>,
    pub(crate) imap: Arc<dyn ImapSync>,
    pub(crate) account_id: AccountId,
    pub(crate) folder_name: String,
    pub(crate) credential: WorkerCredentialSource,
    pub(crate) cancel: CancellationToken,
    pub(crate) config: WorkerConfig,
}

pub(crate) async fn run_polling_worker(worker: PollingWorker) {
    let PollingWorker {
        pool,
        hub,
        imap,
        account_id,
        folder_name,
        credential,
        cancel,
        config,
    } = worker;
    let mut backoff = config.initial_backoff;
    let mut reporter = StateReporter::new(hub.clone(), account_id);
    reporter.transition(SyncState::Polling, None).await;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        let cycle_credential = match credential_for_cycle(
            &credential,
            account_id,
            &folder_name,
            &cancel,
            config,
            &mut backoff,
            "sync worker",
            Some(&mut reporter),
        )
        .await
        {
            CredentialCycle::Ready(credential) => credential,
            CredentialCycle::Retry => continue,
            CredentialCycle::Stop => return,
        };

        reporter.transition(SyncState::Syncing, None).await;
        let result = tokio::select! {
            _ = cancel.cancelled() => return,
            result = reconcile_folder(
                &pool,
                &hub,
                imap.as_ref(),
                account_id,
                &folder_name,
                &cycle_credential,
            ) => result,
        };

        match result {
            Ok(report) => {
                tracing::debug!(
                    %account_id,
                    folder_name = %folder_name,
                    inserted = report.inserted,
                    wiped = report.wiped,
                    "sync worker reconciled folder"
                );
                backoff = config.initial_backoff;
                reporter.transition(SyncState::Polling, None).await;
                if sleep_or_cancel(config.poll_interval, &cancel).await {
                    return;
                }
            }
            Err(err) if is_auth_error(&err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    "sync worker stopped after authentication failure"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    retry_in_ms = backoff.as_millis(),
                    "sync worker reconcile failed; retrying"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                if sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff, config.max_backoff);
            }
        }
    }
}

pub(crate) async fn run_sync_worker(worker: SyncWorker) {
    let SyncWorker {
        pool,
        hub,
        imap,
        idle,
        account_id,
        folder_name,
        credential,
        cancel,
        config,
    } = worker;

    let Some(idle) = idle else {
        run_polling_worker(PollingWorker {
            pool,
            hub,
            imap,
            account_id,
            folder_name,
            credential,
            cancel,
            config,
        })
        .await;
        return;
    };

    let mut backoff = config.initial_backoff;
    let mut reporter = StateReporter::new(hub.clone(), account_id);
    reporter.transition(SyncState::Idle, None).await;
    loop {
        if cancel.is_cancelled() {
            return;
        }

        let cycle_credential = match credential_for_cycle(
            &credential,
            account_id,
            &folder_name,
            &cancel,
            config,
            &mut backoff,
            "idle sync worker",
            Some(&mut reporter),
        )
        .await
        {
            CredentialCycle::Ready(credential) => credential,
            CredentialCycle::Retry => continue,
            CredentialCycle::Stop => return,
        };

        reporter.transition(SyncState::Syncing, None).await;
        let reconcile = tokio::select! {
            _ = cancel.cancelled() => return,
            result = reconcile_folder(
                &pool,
                &hub,
                imap.as_ref(),
                account_id,
                &folder_name,
                &cycle_credential,
            ) => result,
        };

        match reconcile {
            Ok(report) => {
                tracing::debug!(
                    %account_id,
                    folder_name = %folder_name,
                    inserted = report.inserted,
                    wiped = report.wiped,
                    "idle sync worker reconciled folder"
                );
                backoff = config.initial_backoff;
            }
            Err(err) if is_auth_error(&err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    "idle sync worker stopped after authentication failure"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    retry_in_ms = backoff.as_millis(),
                    "idle sync worker reconcile failed; retrying"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                if sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff, config.max_backoff);
                continue;
            }
        }

        let account = match crate::db::accounts::get(&pool, account_id).await {
            Ok(Some(account)) => account,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    retry_in_ms = backoff.as_millis(),
                    "idle sync worker account lookup failed; retrying"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                if sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff, config.max_backoff);
                continue;
            }
        };

        reporter.transition(SyncState::Idle, None).await;
        let wait = idle
            .idle_once(IdleRequest {
                host: &account.imap_host,
                port: account.imap_port as u16,
                username: &account.email,
                credential: &cycle_credential,
                folder: &folder_name,
                timeout: config.idle_timeout,
                cancel: cancel.clone(),
            })
            .await;

        match wait {
            Ok(IdleOutcome::NewData | IdleOutcome::Timeout) => {
                backoff = config.initial_backoff;
            }
            Ok(IdleOutcome::Interrupted) => {
                if cancel.is_cancelled() {
                    return;
                }
                backoff = config.initial_backoff;
            }
            Err(ImapError::Unsupported(_)) => {
                run_polling_worker(PollingWorker {
                    pool,
                    hub,
                    imap,
                    account_id,
                    folder_name,
                    credential,
                    cancel,
                    config,
                })
                .await;
                return;
            }
            Err(err @ ImapError::Auth(_)) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    "idle sync worker stopped after authentication failure"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    retry_in_ms = backoff.as_millis(),
                    "idle sync worker wait failed; retrying"
                );
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
                if sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff, config.max_backoff);
            }
        }
    }
}

fn is_auth_error(err: &SyncError) -> bool {
    matches!(
        err,
        SyncError::Imap(ImapError::Auth(_)) | SyncError::MissingCredentials
    )
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

enum CredentialCycle {
    Ready(MailCredential),
    Retry,
    Stop,
}

#[allow(clippy::too_many_arguments)]
async fn credential_for_cycle(
    source: &WorkerCredentialSource,
    account_id: AccountId,
    folder_name: &str,
    cancel: &CancellationToken,
    config: WorkerConfig,
    backoff: &mut Duration,
    worker_label: &'static str,
    reporter: Option<&mut StateReporter>,
) -> CredentialCycle {
    let result = tokio::select! {
        _ = cancel.cancelled() => return CredentialCycle::Stop,
        result = source.resolve() => result,
    };

    match result {
        Ok(credential) => CredentialCycle::Ready(credential),
        Err(err) if is_auth_error(&err) => {
            tracing::warn!(
                %account_id,
                folder_name = %folder_name,
                error = %err,
                "{worker_label} stopped after credential failure"
            );
            if let Some(reporter) = reporter {
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
            }
            CredentialCycle::Stop
        }
        Err(err) => {
            tracing::warn!(
                %account_id,
                folder_name = %folder_name,
                error = %err,
                retry_in_ms = backoff.as_millis(),
                "{worker_label} credential resolution failed; retrying"
            );
            if let Some(reporter) = reporter {
                reporter
                    .transition(SyncState::Error, Some(err.to_string()))
                    .await;
            }
            if sleep_or_cancel(*backoff, cancel).await {
                return CredentialCycle::Stop;
            }
            *backoff = next_backoff(*backoff, config.max_backoff);
            CredentialCycle::Retry
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::sync::Notify;
    use tokio::task::JoinHandle;
    use tokio::time::{timeout, Duration};

    use crate::db::{accounts, folders};
    use crate::imap::FolderSync;
    use crate::models::{AuthKind, FolderRole};

    use super::*;

    #[derive(Debug, Clone)]
    enum Outcome {
        Ok,
        Protocol,
        Auth,
    }

    struct ScriptedSync {
        calls: AtomicUsize,
        outcomes: Mutex<VecDeque<Outcome>>,
    }

    impl ScriptedSync {
        fn new(outcomes: Vec<Outcome>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes.into()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ImapSync for ScriptedSync {
        async fn sync_folder(
            &self,
            _: &str,
            _: u16,
            _: &str,
            credential: &MailCredential,
            _: &str,
            _: u32,
        ) -> Result<FolderSync, ImapError> {
            assert_eq!(credential.secret(), "right");
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Outcome::Ok)
            {
                Outcome::Ok => Ok(FolderSync {
                    uid_validity: Some(1),
                    uid_next: Some(1),
                    exists: 0,
                    messages: vec![],
                }),
                Outcome::Protocol => Err(ImapError::Protocol("temporary failure".into())),
                Outcome::Auth => Err(ImapError::Auth("bad password".into())),
            }
        }
    }

    #[derive(Debug, Clone)]
    enum IdleScript {
        WaitForSignal,
        Unsupported,
    }

    struct ScriptedIdle {
        calls: AtomicUsize,
        outcomes: Mutex<VecDeque<IdleScript>>,
        entered: Notify,
        signal: Notify,
    }

    impl ScriptedIdle {
        fn new(outcomes: Vec<IdleScript>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes.into()),
                entered: Notify::new(),
                signal: Notify::new(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        async fn wait_until_entered(&self, expected: usize) {
            timeout(Duration::from_secs(1), async {
                loop {
                    if self.calls() >= expected {
                        return;
                    }
                    self.entered.notified().await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[async_trait::async_trait]
    impl ImapIdle for ScriptedIdle {
        async fn idle_once(&self, request: IdleRequest<'_>) -> Result<IdleOutcome, ImapError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_waiters();
            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(IdleScript::WaitForSignal);
            match outcome {
                IdleScript::Unsupported => Err(ImapError::Unsupported("no idle".into())),
                IdleScript::WaitForSignal => {
                    tokio::select! {
                        _ = self.signal.notified() => Ok(IdleOutcome::NewData),
                        _ = request.cancel.cancelled() => Ok(IdleOutcome::Interrupted),
                    }
                }
            }
        }
    }

    fn fast_config() -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(20),
            idle_timeout: Duration::from_secs(30),
            initial_backoff: Duration::from_millis(5),
            max_backoff: Duration::from_millis(10),
        }
    }

    async fn seed_account_folder(pool: &SqlitePool) -> AccountId {
        let account = accounts::create(
            pool,
            &accounts::NewAccount {
                email: "u@example.com".into(),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "imap.example.com".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "smtp.example.com".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        folders::upsert(
            pool,
            &folders::NewFolder {
                account_id: account.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
        account.id
    }

    fn spawn_test_worker(
        pool: SqlitePool,
        sync: Arc<ScriptedSync>,
        account_id: AccountId,
        config: WorkerConfig,
    ) -> (CancellationToken, JoinHandle<()>) {
        spawn_test_worker_with_hub(pool, sync, account_id, config, Arc::new(Hub::new()))
    }

    fn spawn_test_worker_with_hub(
        pool: SqlitePool,
        sync: Arc<ScriptedSync>,
        account_id: AccountId,
        config: WorkerConfig,
        hub: Arc<Hub>,
    ) -> (CancellationToken, JoinHandle<()>) {
        let cancel = CancellationToken::new();
        let imap: Arc<dyn ImapSync> = sync;
        let handle = tokio::spawn(run_polling_worker(PollingWorker {
            pool,
            hub,
            imap,
            account_id,
            folder_name: "INBOX".into(),
            credential: WorkerCredentialSource::static_credential(MailCredential::password(
                "right",
            )),
            cancel: cancel.clone(),
            config,
        }));
        (cancel, handle)
    }

    fn spawn_idle_worker(
        pool: SqlitePool,
        sync: Arc<ScriptedSync>,
        idle: Arc<ScriptedIdle>,
        account_id: AccountId,
        config: WorkerConfig,
    ) -> (CancellationToken, JoinHandle<()>) {
        let cancel = CancellationToken::new();
        let imap: Arc<dyn ImapSync> = sync;
        let idle_trait: Arc<dyn ImapIdle> = idle;
        let handle = tokio::spawn(run_sync_worker(SyncWorker {
            pool,
            hub: Arc::new(Hub::new()),
            imap,
            idle: Some(idle_trait),
            account_id,
            folder_name: "INBOX".into(),
            credential: WorkerCredentialSource::static_credential(MailCredential::password(
                "right",
            )),
            cancel: cancel.clone(),
            config,
        }));
        (cancel, handle)
    }

    async fn collect_sync_states(
        rx: &mut tokio::sync::broadcast::Receiver<Arc<serde_json::Value>>,
        budget: Duration,
    ) -> Vec<SyncStateEvent> {
        let mut events = Vec::new();
        let _ = timeout(budget, async {
            while let Ok(payload) = rx.recv().await {
                if let Ok(event) = serde_json::from_value::<SyncStateEvent>((*payload).clone()) {
                    events.push(event);
                }
            }
        })
        .await;
        events
    }

    async fn wait_for_calls(sync: &ScriptedSync, expected: usize) {
        timeout(Duration::from_secs(1), async {
            loop {
                if sync.calls() >= expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_worker_polls_at_configured_interval() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 3).await;

        cancel.cancel();
        handle.await.unwrap();
        assert!(sync.calls() >= 3);
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancel_and_no_further_syncs_happen() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 1).await;
        cancel.cancel();
        timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let after_cancel = sync.calls();

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(sync.calls(), after_cancel);
    }

    #[tokio::test]
    async fn test_worker_recovers_from_transient_protocol_error() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Protocol, Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 2).await;

        cancel.cancel();
        handle.await.unwrap();
        assert!(sync.calls() >= 2);
    }

    #[tokio::test]
    async fn test_worker_terminates_on_auth_error() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Auth]));
        let (_cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let after_auth = sync.calls();

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(after_auth, 1);
        assert_eq!(sync.calls(), after_auth);
    }

    #[tokio::test]
    async fn test_idle_worker_reconciles_initially_and_after_idle_change() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let idle = Arc::new(ScriptedIdle::new(vec![IdleScript::WaitForSignal]));
        let (cancel, handle) =
            spawn_idle_worker(pool, sync.clone(), idle.clone(), account_id, fast_config());

        wait_for_calls(&sync, 1).await;
        idle.wait_until_entered(1).await;
        idle.signal.notify_waiters();
        wait_for_calls(&sync, 2).await;

        cancel.cancel();
        handle.await.unwrap();
        assert!(idle.calls() >= 1);
    }

    #[tokio::test]
    async fn test_idle_worker_falls_back_to_polling_when_idle_unsupported() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let idle = Arc::new(ScriptedIdle::new(vec![IdleScript::Unsupported]));
        let (cancel, handle) =
            spawn_idle_worker(pool, sync.clone(), idle.clone(), account_id, fast_config());

        wait_for_calls(&sync, 3).await;

        cancel.cancel();
        handle.await.unwrap();
        assert_eq!(idle.calls(), 1);
    }

    #[tokio::test]
    async fn test_idle_worker_cancel_interrupts_idle_wait_and_joins_cleanly() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let idle = Arc::new(ScriptedIdle::new(vec![IdleScript::WaitForSignal]));
        let (cancel, handle) =
            spawn_idle_worker(pool, sync.clone(), idle.clone(), account_id, fast_config());

        wait_for_calls(&sync, 1).await;
        idle.wait_until_entered(1).await;
        cancel.cancel();
        timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let after_cancel = sync.calls();

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(sync.calls(), after_cancel);
        assert_eq!(idle.calls(), 1);
    }

    #[tokio::test]
    async fn test_polling_worker_publishes_polling_syncing_states() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let hub = Arc::new(Hub::new());
        let mut rx = hub.subscribe(crate::ipc::Topic::SyncState).await;
        let (cancel, handle) =
            spawn_test_worker_with_hub(pool, sync.clone(), account_id, fast_config(), hub.clone());

        wait_for_calls(&sync, 2).await;
        cancel.cancel();
        handle.await.unwrap();

        let events = collect_sync_states(&mut rx, Duration::from_millis(50)).await;
        let states: Vec<_> = events.iter().map(|e| e.state).collect();
        assert!(
            states.contains(&SyncState::Polling),
            "expected polling state in {states:?}"
        );
        assert!(
            states.contains(&SyncState::Syncing),
            "expected syncing state in {states:?}"
        );
        assert!(events.iter().all(|e| e.account_id == account_id));
    }

    #[tokio::test]
    async fn test_polling_worker_publishes_error_state_with_last_error() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Protocol, Outcome::Ok]));
        let hub = Arc::new(Hub::new());
        let mut rx = hub.subscribe(crate::ipc::Topic::SyncState).await;
        let (cancel, handle) =
            spawn_test_worker_with_hub(pool, sync.clone(), account_id, fast_config(), hub.clone());

        wait_for_calls(&sync, 2).await;
        cancel.cancel();
        handle.await.unwrap();

        let events = collect_sync_states(&mut rx, Duration::from_millis(50)).await;
        let error_events: Vec<_> = events
            .iter()
            .filter(|e| e.state == SyncState::Error)
            .collect();
        assert!(
            !error_events.is_empty(),
            "expected at least one error state event in {events:?}"
        );
        assert!(error_events
            .iter()
            .all(|e| e.last_error.as_deref().is_some_and(|m| !m.is_empty())));

        // After recovery, a non-error state must follow with last_error == None.
        let non_error_after_error = events
            .iter()
            .skip_while(|e| e.state != SyncState::Error)
            .find(|e| e.state != SyncState::Error);
        assert!(
            non_error_after_error.is_some_and(|e| e.last_error.is_none()),
            "expected non-error transition after error to clear last_error: {events:?}"
        );
    }
}
