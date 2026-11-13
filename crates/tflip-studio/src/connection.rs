use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tflip_engine::Error;
use tflip_engine::connection;
use tflip_protocol::{Api, Handle, Session};

use crate::error::ApiError;

pub struct Connection {
    api: Box<dyn Api + Send + Sync>,
    handle: Handle,
}

impl Connection {
    /// Every `Api` call may block on disk or network, so none runs on
    /// tokio's worker threads.
    pub async fn call<R: Send + 'static>(
        self: Arc<Self>,
        f: impl FnOnce(&dyn Api, Handle) -> Result<R, Error> + Send + 'static,
    ) -> Result<R, ApiError> {
        let joined = tokio::task::spawn_blocking(move || f(&*self.api, self.handle)).await;
        joined
            .expect("an Api call panicked")
            .map_err(ApiError::from)
    }
}

#[derive(Default)]
pub struct Connections {
    next: AtomicU32,
    open: Mutex<HashMap<u32, Arc<Connection>>>,
}

impl Connections {
    pub async fn open_local(
        &self,
        path: String,
        user: Option<String>,
        password: Option<String>,
    ) -> Result<u32, ApiError> {
        let connection = tokio::task::spawn_blocking(move || {
            let session = Session::new();
            let handle = session.connect_path(path, connection::LOCAL, user, password, None)?;
            Ok::<_, Error>(Connection {
                api: Box::new(session),
                handle,
            })
        })
        .await
        .expect("connecting panicked")?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.open.lock().unwrap().insert(id, Arc::new(connection));
        Ok(id)
    }

    pub fn get(&self, id: u32) -> Result<Arc<Connection>, ApiError> {
        let open = self.open.lock().unwrap();
        open.get(&id)
            .cloned()
            .ok_or_else(|| Error::InvalidHandle.into())
    }

    pub async fn close(&self, id: u32) -> Result<(), ApiError> {
        let connection = self.open.lock().unwrap().remove(&id);
        let connection = connection.ok_or(ApiError::from(Error::InvalidHandle))?;
        // Dropping the session writes what is pending and rolls back open
        // transactions: disk work, like any call.
        tokio::task::spawn_blocking(move || drop(connection))
            .await
            .expect("closing panicked");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tflip_engine::ace_error::codes::AE_INVALID_HANDLE;

    use super::*;

    /// As a connection reports it, which on Windows is the full path.
    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/adt")
            .join(name);
        tflip_storage::universal_path(path.to_str().unwrap())
    }

    async fn open(connections: &Connections) -> u32 {
        connections
            .open_local(fixture("all_types"), None, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn opens_a_folder_and_calls_on_it() {
        let connections = Connections::default();
        let id = open(&connections).await;
        let connection = connections.get(id).unwrap();
        let info = connection
            .call(|api, handle| api.connection_info(handle))
            .await
            .unwrap();
        assert_eq!(info.path, fixture("all_types"));
        assert!(!info.dictionary);
    }

    #[tokio::test]
    async fn forgets_a_closed_connection() {
        let connections = Connections::default();
        let id = open(&connections).await;
        connections.close(id).await.unwrap();
        assert_eq!(connections.get(id).err().unwrap().code, AE_INVALID_HANDLE);
        assert_eq!(
            connections.close(id).await.unwrap_err().code,
            AE_INVALID_HANDLE
        );
    }

    #[tokio::test]
    async fn opens_a_dictionary() {
        let connections = Connections::default();
        let path = fixture("dictionary/dict.add");
        let user = Some("ADSSYS".to_owned());
        let password = Some("adssyspw".to_owned());
        let id = connections
            .open_local(path.clone(), user, password)
            .await
            .unwrap();
        let info = connections
            .get(id)
            .unwrap()
            .call(|api, handle| api.connection_info(handle))
            .await
            .unwrap();
        assert_eq!(info.path, path);
        assert!(info.dictionary);
        assert_eq!(info.user.as_deref(), Some("ADSSYS"));
    }

    /// A dictionary rather than a folder: like the original local server,
    /// the engine does not check that a folder exists.
    #[tokio::test]
    async fn reports_a_missing_dictionary() {
        let connections = Connections::default();
        let missing = format!("{}/missing.add", fixture("all_types"));
        let error = connections
            .open_local(missing, None, None)
            .await
            .unwrap_err();
        assert_ne!(error.code, AE_INVALID_HANDLE);
        assert!(connections.open.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn keeps_connections_apart() {
        let connections = Connections::default();
        let first = open(&connections).await;
        let second = open(&connections).await;
        assert_ne!(first, second);
        connections.close(first).await.unwrap();
        let info = connections
            .get(second)
            .unwrap()
            .call(|api, handle| api.connection_info(handle))
            .await;
        assert!(info.is_ok());
    }
}
