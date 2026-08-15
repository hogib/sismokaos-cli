mod cli;
mod config;
mod engine;
mod export;
mod features;
mod types;

use clap::Parser;
use std::io::Write;
use std::sync::mpsc;
use std::thread;

use cli::{Cli, Commands};
use config::AppConfig;
use types::PipelineEvent;

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { out } => {
            let default_config = AppConfig::default();
            if let Err(e) = default_config.save_to_json(out) {
                eprintln!("[ERROR] Failed to save config: {}", e);
            } else {
                println!("[SUCCESS] Created default configuration at {:?}", out);
            }
        }
        Commands::Run { dry_run, .. } => {
            let mut app_config = AppConfig::from_json(&cli.config);

            // This now works perfectly because everything is in the same crate
            app_config.merge_with_cli(&cli);

            if *dry_run {
                println!("[DRY RUN ENABLED] Exiting without processing.");
                return;
            }

            println!("=== STARTING PIPELINE ===");

            let (tx, rx) = mpsc::channel();
            let engine_config = app_config.clone();

            // Spawn the processing engine in a background thread
            let engine_handle = thread::spawn(move || {
                // We call the engine module here
                if let Err(e) = engine::run_pipeline(engine_config, tx) {
                    eprintln!("Pipeline Error: {}", e);
                }
            });

            // Main UI thread: Listen for events and print progress
            let mut processed_windows = 0;
            let mut current_total_windows = 0;

            for event in rx {
                match event {
                    PipelineEvent::FolderStarted(folder) => {
                        println!("\n Folder: {}", folder);
                    }
                    PipelineEvent::FileStarted {
                        path,
                        total_windows,
                    } => {
                        println!(" File: {:?}", path.file_name().unwrap_or_default());
                        current_total_windows = total_windows;
                        processed_windows = 0;
                        print!(" Progress: [");
                    }
                    PipelineEvent::WindowProcessed => {
                        processed_windows += 1;
                        print!("#");
                        let _ = std::io::stdout().flush();
                    }
                    PipelineEvent::FileCompleted => {
                        println!("] {}/{} windows", processed_windows, current_total_windows);
                    }
                    PipelineEvent::Warning(msg) => {
                        println!("\n  Warning: {}", msg);
                    }
                    PipelineEvent::Finished => {
                        println!("\nAll processing finished successfully");
                    }
                }
            }

            let _ = engine_handle.join();
        }
    }
}
