use std::path::Path;

pub async fn try_create_dir(path: &Path) -> std::io::Result<()> {
    let create_result = tokio::fs::create_dir(path).await;
    match create_result {
        Ok(_) => Ok(()),
        Err(error) => {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}
