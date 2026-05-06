pub mod app;
pub mod ipc;
pub mod render;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use thiserror::Error;

use app::{ActivePane, AppState};
use ipc::MailboxClient;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("unable to connect to daemon socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: ipc::MailboxError,
    },
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
}

type CrosstermTerminal = Terminal<CrosstermBackend<Stdout>>;

pub async fn run(socket_path: PathBuf) -> Result<(), TuiError> {
    let mut client = MailboxClient::connect(&socket_path)
        .await
        .map_err(|source| TuiError::Connect {
            path: socket_path.clone(),
            source,
        })?;
    let mut app = AppState::default();
    app.set_status(format!("Connected to {}", socket_path.display()));
    refresh_accounts(&mut app, &mut client).await;

    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, app, client).await;
    let restore_result = restore_terminal(&mut terminal);

    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn run_loop(
    terminal: &mut CrosstermTerminal,
    mut app: AppState,
    mut client: MailboxClient,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| render::render(frame, &app))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if handle_key(key, &mut app, &mut client).await {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

async fn handle_key(key: KeyEvent, app: &mut AppState, client: &mut MailboxClient) -> bool {
    match key.code {
        KeyCode::Char('q') => true,
        KeyCode::Down | KeyCode::Char('j') => {
            if app.move_selection(1) {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.move_selection(-1) {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        KeyCode::Tab => {
            app.cycle_active_pane();
            false
        }
        KeyCode::Char('r') => {
            refresh_current_pane(app, client).await;
            false
        }
        KeyCode::Enter => {
            refresh_after_selection_change(app, client).await;
            false
        }
        _ => false,
    }
}

async fn refresh_current_pane(app: &mut AppState, client: &mut MailboxClient) {
    match app.active {
        ActivePane::Accounts => refresh_accounts(app, client).await,
        ActivePane::Folders => refresh_folders(app, client).await,
        ActivePane::Messages => refresh_messages(app, client).await,
    }
}

async fn refresh_after_selection_change(app: &mut AppState, client: &mut MailboxClient) {
    match app.active {
        ActivePane::Accounts => refresh_folders(app, client).await,
        ActivePane::Folders => refresh_messages(app, client).await,
        ActivePane::Messages => refresh_detail(app, client).await,
    }
}

async fn refresh_accounts(app: &mut AppState, client: &mut MailboxClient) {
    app.set_status("Loading accounts");
    match client.list_accounts().await {
        Ok(accounts) => {
            let count = accounts.len();
            app.clear_error();
            app.apply_accounts(accounts);
            if count == 0 {
                app.set_status("Connected. No accounts found");
            } else {
                app.set_status(format!("Loaded {count} account(s)"));
                refresh_folders(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_folders(app: &mut AppState, client: &mut MailboxClient) {
    let Some(account_id) = app.selected_account_id() else {
        app.apply_folders(Vec::new());
        app.set_status("No account selected");
        return;
    };

    app.set_status("Loading folders");
    match client.list_folders(account_id).await {
        Ok(folders) => {
            let count = folders.len();
            app.clear_error();
            app.apply_folders(folders);
            if count == 0 {
                app.set_status("No folders for selected account");
            } else {
                app.set_status(format!("Loaded {count} folder(s)"));
                refresh_messages(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_messages(app: &mut AppState, client: &mut MailboxClient) {
    let Some(folder_id) = app.selected_folder_id() else {
        app.apply_messages(Vec::new());
        app.set_status("No folder selected");
        return;
    };

    app.set_status("Loading messages");
    match client.list_messages(folder_id).await {
        Ok(messages) => {
            let count = messages.len();
            app.clear_error();
            app.apply_messages(messages);
            if count == 0 {
                app.set_status("No messages in selected folder");
            } else {
                app.set_status(format!("Loaded {count} message(s)"));
                refresh_detail(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_detail(app: &mut AppState, client: &mut MailboxClient) {
    let Some(message_id) = app.selected_message_id() else {
        app.apply_detail(None);
        app.set_status("No message selected");
        return;
    };

    app.set_status("Loading message");
    match client.get_message(message_id).await {
        Ok(detail) => {
            app.clear_error();
            if detail.is_some() {
                app.set_status("Message loaded");
            } else {
                app.set_status("Message no longer exists");
            }
            app.apply_detail(detail);
        }
        Err(error) => record_error(app, error),
    }
}

fn record_error(app: &mut AppState, error: ipc::MailboxError) {
    let message = error.to_string();
    app.set_status(message.clone());
    app.set_error(message);
}

fn setup_terminal() -> Result<CrosstermTerminal, TuiError> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut CrosstermTerminal) -> Result<(), TuiError> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
