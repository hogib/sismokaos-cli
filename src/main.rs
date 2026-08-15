mod cli;
mod config;
mod engine;
mod export;
mod features;
mod preprocess;
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
            app_config.merge_with_cli(&cli);

            if *dry_run {
                println!("[DRY RUN ENABLED] Exiting without processing.");
                return;
            }

            println!("=== STARTING PIPELINE ===");

            // Unbounded channel is perfect here because UI prints take virtually zero time
            let (tx, rx) = mpsc::channel();
            let engine_config = app_config.clone();

            // Background Thread: Heavy compute (I/O, Filtering, Windowing)
            let engine_handle = thread::spawn(move || {
                if let Err(e) = engine::run_pipeline(engine_config, tx) {
                    eprintln!("Pipeline Error: {}", e);
                }
            });

            // Main Thread: Asynchronous UI consumption
            let mut processed_windows: u64 = 0;

            for event in rx {
                match event {
                    PipelineEvent::FolderStarted(folder) => {
                        println!(" Input: {}", folder);
                    }
                    PipelineEvent::WindowProcessed => {
                        processed_windows += 1;
                        if processed_windows % 120 == 0 {
                            print!("\r Windows processed: {}", processed_windows);
                            let _ = std::io::stdout().flush();
                        }
                    }
                    PipelineEvent::Completed => {
                        println!("\r Windows processed: {}", processed_windows);
                    }
                    PipelineEvent::Warning(msg) => {
                        println!("\r Warning: {}", msg);
                    }
                    PipelineEvent::Finished => {
                        println!("All processing finished successfully");
                    }
                }
            }

            let _ = engine_handle.join();
        }
    }
}
