use crate::error::IndexingError;

pub fn handle_task_result(
    result: Result<Result<(), IndexingError>, tokio::task::JoinError>,
) -> Result<(), IndexingError> {
    match result {
        Ok(handler_result) => handler_result,
        Err(join_error) => Err(IndexingError::from(join_error)),
    }
}