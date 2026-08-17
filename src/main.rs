mod cli;
mod config;
mod engine;
mod export;
mod features;
mod preprocess;
mod preprocess_only;
mod raw_binary;
mod types;

use clap::Parser;
use std::io::Write;
use std::sync::mpsc;
use std::thread;

use cli::{Cli, Commands};
use config::AppConfig;
use mimalloc::MiMalloc;
use types::PipelineEvent;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
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
        Commands::Run { dry_run, .. } | Commands::Preprocess { dry_run, .. } => {
            let mut app_config = AppConfig::from_json(&cli.config);
            app_config.merge_with_cli(&cli);

            // Checked before any work starts. These are combinations whose
            // damage is invisible in the output, so failing late would mean
            // shipping a corrupt dataset that looks perfectly fine.
            let problems = app_config.validate();
            if !problems.is_empty() {
                eprintln!("[ERROR] Invalid configuration:");
                for p in &problems {
                    eprintln!("  - {}", p);
                }
                std::process::exit(2);
            }

            if *dry_run {
                println!("[DRY RUN ENABLED] Exiting without processing.");
                return;
            }

            println!("=== STARTING PIPELINE ===");

            let (tx, rx) = mpsc::channel();
            let engine_config = app_config.clone();

            let run_preprocess = matches!(&cli.command, Commands::Preprocess { .. });

            let engine_handle = thread::spawn(move || {
                let result = if run_preprocess {
                    preprocess_only::run_preprocess_only(engine_config, tx)
                } else {
                    engine::run_pipeline(engine_config, tx)
                };

                if let Err(e) = result {
                    eprintln!("Pipeline Error: {}", e);
                }
            });

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
                    PipelineEvent::ChunkProcessed(sample_count) => {
                        processed_windows += sample_count as u64;
                        if processed_windows % 100_000 < sample_count as u64 {
                            print!("\r Samples processed: {}", processed_windows);
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
