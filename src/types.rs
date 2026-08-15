use std::path::PathBuf;

/// Represents an event emitted by the background processing pipeline
#[derive(Debug)]
pub enum PipelineEvent {
    /// Started processing a new date folder
    FolderStarted(String),
    /// Started processing a specific file (includes total windows to process)
    FileStarted { path: PathBuf, total_windows: usize },
    /// Successfully computed a single window
    WindowProcessed,
    /// Finished processing a file
    FileCompleted,
    /// Non-fatal error or warning during processing
    Warning(String),
    /// Pipeline finished completely
    Finished,
}

/// A single window of 3-channel data ready for feature extraction
#[derive(Debug, Clone)]
pub struct WindowData {
    pub e: Vec<f64>,
    pub n: Vec<f64>,
    pub z: Vec<f64>,
    pub window_id: String,
    pub time_minutes: f64,
}
