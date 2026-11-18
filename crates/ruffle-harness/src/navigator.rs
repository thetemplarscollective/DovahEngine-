//! Stub `NavigatorBackend` (M1): serves the synthesized
//! `gfxfontlib.swf` from memory; every other fetch falls back to the
//! local filesystem, exactly like Ruffle's `NullNavigatorBackend`.
//!
//! Why the fontlib needs interception: every Skyrim menu imports its
//! fonts from `gfxfontlib.swf` via `ImportAssets2`, but vanilla
//! Skyrim ships no such file — the game registers `fonts_en.swf`
//! through `fontconfig.txt` and Scaleform intercepts the URL
//! internally (feasibility §4.1). The study synthesized a
//! `gfxfontlib.swf` (12 `DefineFont3` tags from `fonts_en.swf` +
//! one `ExportAssets`); this backend serves those bytes from memory
//! whenever the movie asks for that URL, so no file has to sit next
//! to the menu SWF.
//!
//! Futures: Ruffle's loader spawns async work through
//! [`NavigatorBackend::spawn_future`]. The harness owns a
//! [`NullExecutor`] and pumps it once per frame
//! ([`NullExecutor::run`]); this backend only holds the
//! [`NullSpawner`] handle, mirroring `NullNavigatorBackend`.

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use encoding_rs::Encoding;
use ruffle_core::backend::navigator::{
    async_return, fetch_path, resolve_url_with_relative_base_path, ErrorResponse, NavigationMethod,
    NavigatorBackend, NullExecutor, NullSpawner, OwnedFuture, Request, SuccessResponse,
};
use ruffle_core::indexmap::IndexMap;
use ruffle_core::loader::Error;
use ruffle_core::socket::{ConnectionState, SocketAction, SocketHandle};
use url::{ParseError, Url};

/// The font-library filename the menus import. Matched
/// case-insensitively against the last path segment of the fetch URL
/// (the import URL is `gfxfontlib.swf`; after resolution it is a
/// `file://` URL ending in the same name).
pub const FONTLIB_FILENAME: &str = "gfxfontlib.swf";

/// True when `url` asks for the font library, in either its relative
/// form (`gfxfontlib.swf`) or its resolved `file://` form.
pub fn is_fontlib_url(url: &str) -> bool {
    url.rsplit('/')
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case(FONTLIB_FILENAME))
}

/// A [`SuccessResponse`] backed by bytes already in memory.
pub struct MemoryResponse {
    url: String,
    data: Option<Vec<u8>>,
}

impl MemoryResponse {
    pub fn new(url: String, data: Vec<u8>) -> Self {
        Self {
            url,
            data: Some(data),
        }
    }
}

impl SuccessResponse for MemoryResponse {
    fn url(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, Error> {
        let data = self.data.unwrap_or_default();
        async_return(Ok(data))
    }

    fn text_encoding(&self) -> Option<&'static Encoding> {
        None
    }

    fn status(&self) -> u16 {
        200
    }

    fn redirected(&self) -> bool {
        false
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, Error> {
        let chunk = self.data.take();
        async_return(Ok(chunk))
    }

    fn expected_length(&self) -> Result<Option<u64>, Error> {
        Ok(self.data.as_ref().map(|d| d.len() as u64))
    }
}

/// The M1 navigator: fontlib from memory, everything else from disk.
pub struct MemoryNavigatorBackend {
    spawner: NullSpawner,
    base_path: PathBuf,
    fontlib: Vec<u8>,
}

impl MemoryNavigatorBackend {
    /// `fontlib` is the synthesized `gfxfontlib.swf` bytes;
    /// `base_path` is the directory relative fetches resolve against
    /// (the menu SWF's directory); `executor` is the harness-owned
    /// executor, pumped once per frame — only its spawner handle is
    /// kept here.
    ///
    /// The base path is canonicalized: `resolve_url` builds a
    /// `file://` base from it via `Url::from_file_path`, which
    /// REFUSES relative paths, so a relative base made every
    /// non-fontlib relative fetch fail with "relative URL without
    /// a base" (M3: the inventory menu's component imports died
    /// on exactly this, the root movie then sat in preload
    /// forever, and the frame stayed white). Falls back to the
    /// raw path if the directory does not exist, so fetches fail
    /// cleanly as before instead of the constructor panicking.
    pub fn new(fontlib: Vec<u8>, base_path: PathBuf, executor: &NullExecutor) -> Self {
        let base_path = std::fs::canonicalize(&base_path).unwrap_or(base_path);
        Self {
            spawner: executor.spawner(),
            base_path,
            fontlib,
        }
    }
}

impl NavigatorBackend for MemoryNavigatorBackend {
    fn navigate_to_url(
        &self,
        _url: &str,
        _target: &str,
        _vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        // Menus never navigate; the harness has nowhere to go.
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let url = request.url().to_string();
        if is_fontlib_url(&url) {
            let response: Box<dyn SuccessResponse> =
                Box::new(MemoryResponse::new(url, self.fontlib.clone()));
            return async_return(Ok(response));
        }
        // Not the fontlib: read from disk, like NullNavigatorBackend.
        // A missing file yields a clean fetch error, not a panic.
        fetch_path(
            self,
            "MemoryNavigatorBackend",
            &url,
            Some(self.base_path.as_path()),
        )
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        resolve_url_with_relative_base_path(self, self.base_path.clone(), url)
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.spawner.spawn_local(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        url
    }

    fn connect_socket(
        &mut self,
        _host: String,
        _port: u16,
        _timeout: Duration,
        handle: SocketHandle,
        _receiver: Receiver<Vec<u8>>,
        sender: Sender<SocketAction>,
    ) {
        // Menus open no sockets; fail fast, like NullNavigatorBackend.
        sender
            .try_send(SocketAction::Connect(handle, ConnectionState::Failed))
            .expect("working channel send");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[test]
    fn fontlib_url_forms_match() {
        assert!(is_fontlib_url("gfxfontlib.swf"));
        assert!(is_fontlib_url("file:///home/hatch/x/gfxfontlib.swf"));
        assert!(is_fontlib_url("GFXFONTLIB.SWF"));
        assert!(is_fontlib_url("./gfxfontlib.swf"));
        assert!(!is_fontlib_url("hudmenu.gfx"));
        assert!(!is_fontlib_url("gfxfontlib.swf.bak"));
        assert!(!is_fontlib_url("notgfxfontlib.swf"));
        assert!(!is_fontlib_url(""));
    }

    #[test]
    fn memory_response_body_returns_the_bytes() {
        let response: Box<dyn SuccessResponse> = Box::new(MemoryResponse::new(
            "gfxfontlib.swf".into(),
            vec![1, 2, 3, 4],
        ));
        assert_eq!(response.status(), 200);
        assert!(!response.redirected());
        assert_eq!(response.expected_length().unwrap(), Some(4));
        let body = block_on(response.body()).unwrap();
        assert_eq!(body, vec![1, 2, 3, 4]);
    }

    #[test]
    fn memory_response_chunks_yield_once_then_none() {
        let mut response = MemoryResponse::new("gfxfontlib.swf".into(), vec![9, 9]);
        let first = block_on(response.next_chunk()).unwrap();
        assert_eq!(first, Some(vec![9, 9]));
        let second = block_on(response.next_chunk()).unwrap();
        assert_eq!(second, None);
    }

    #[test]
    fn fetch_serves_fontlib_from_memory() {
        let executor = NullExecutor::new();
        let backend =
            MemoryNavigatorBackend::new(vec![7, 7, 7], PathBuf::from("/nonexistent"), &executor);
        let response = block_on(backend.fetch(Request::get("gfxfontlib.swf".to_string())))
            .ok()
            .expect("fontlib fetch succeeds");
        assert_eq!(response.status(), 200);
        let body = block_on(response.body()).unwrap();
        assert_eq!(body, vec![7, 7, 7]);
    }

    #[test]
    fn fetch_of_missing_file_fails_on_body() {
        // `fetch_path` returns a response object even for a missing
        // file — the failure surfaces when the body is read. This
        // test pins that behavior so the harness never mistakes a
        // returned response for a successful load.
        let executor = NullExecutor::new();
        let backend = MemoryNavigatorBackend::new(vec![], PathBuf::from("/nonexistent"), &executor);
        let response = block_on(backend.fetch(Request::get("no-such-file.swf".to_string())))
            .ok()
            .expect("fetch_path returns a response object even for a missing file");
        let body = block_on(response.body());
        assert!(body.is_err());
    }

    #[test]
    fn resolve_url_works_with_a_relative_base_path() {
        // Regression test for the M3 white-frame stall: the harness
        // passes the menu SWF's directory as a RELATIVE path, and
        // `Url::from_file_path` refuses relative paths — so the
        // inventory menu's component imports failed with "relative
        // URL without a base", `finish_importing` never ran, and
        // the root movie never ran frame 1. The constructor
        // canonicalizes the base, so relative fetches (including
        // ones with a space in the path, like the real component
        // URLs) resolve to absolute `file://` URLs.
        let executor = NullExecutor::new();
        let backend = MemoryNavigatorBackend::new(vec![], PathBuf::from("."), &executor);
        let url = backend
            .resolve_url("Inventory components/ItemCard.swf")
            .expect("relative fetch resolves against a relative base path");
        assert_eq!(url.scheme(), "file");
        assert!(
            url.as_str()
                .ends_with("Inventory%20components/ItemCard.swf"),
            "unexpected resolved URL: {url}"
        );
    }
}
