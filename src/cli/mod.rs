pub mod doctor;
pub mod init;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "postblox", about = "Self-hosted email infrastructure for AI agents")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Interactive setup wizard — generates postblox.toml and creates first org
    Init(Box<init::InitArgs>),
    /// Diagnose config, database, and service connectivity
    Doctor(doctor::DoctorArgs),
}

pub(crate) fn generate_api_key() -> crate::api::api_keys::GeneratedKey {
    crate::api::api_keys::generate_api_key()
}
