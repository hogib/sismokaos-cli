/// Represents an event emitted by the background processing pipeline.
///
/// The pipeline streams: it discovers how much data there is as it goes, so events report
/// progress made rather than progress against a known total.
#[derive(Debug)]
pub enum PipelineEvent {
    /// Started processing the input directory
    FolderStarted(String),
    /// Successfully computed and wrote a single window
    WindowProcessed,
    /// All windows have been written
    Completed,
    /// Non-fatal error or warning during processing
    Warning(String),
    /// Pipeline finished completely
    Finished,
}
