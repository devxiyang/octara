use crate::media::{MediaFetchRequest, MediaFetchResponse, MediaTransport};
use block2::DynBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLRequest, NSURLRequestCachePolicy, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSURLSessionDataDelegate, NSURLSessionDataTask, NSURLSessionDelegate,
    NSURLSessionResponseDisposition, NSURLSessionTask, NSURLSessionTaskDelegate, ns_string,
};
use octara::runtime::media::{MediaProgress, probe_image_size};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

const REQUEST_IDLE_SECONDS: f64 = 10.0;
const RESOURCE_SECONDS: f64 = 30.0;
const REDIRECT_LIMIT: usize = 8;
const PROBE_LIMIT: usize = 16 * 1024;
const CANCELLED: &str = "Image request was cancelled";
const RESPONSE_TOO_LARGE: &str = "Image response exceeds the byte limit";
const REQUEST_FAILED: &str = "Image HTTP request failed";

type FetchResult = Result<MediaFetchResponse, Arc<str>>;

pub(crate) struct AppleMediaTransport;

impl MediaTransport for AppleMediaTransport {
    fn fetch(&self, request: MediaFetchRequest, cancel: &AtomicBool) -> FetchResult {
        if cancel.load(Ordering::Acquire) || request.cancellation.flag().load(Ordering::Acquire) {
            return Err(CANCELLED.into());
        }
        autoreleasepool(|_| fetch(request, cancel))
    }
}

fn fetch(request: MediaFetchRequest, cancel: &AtomicBool) -> FetchResult {
    let url = NSURL::URLWithString(&NSString::from_str(&request.uri)).ok_or("Image URI is invalid")?;
    if !is_public_http(&url) {
        return Err("Native image URLs must use HTTP or HTTPS without URL credentials".into());
    }

    let url_request = NSMutableURLRequest::requestWithURL_cachePolicy_timeoutInterval(
        &url,
        NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
        REQUEST_IDLE_SECONDS,
    );
    // Let URLSession negotiate HTTP/3, proxy routing, and TLS using system settings.
    for (name, value) in &request.headers {
        if !valid_header(name, value) {
            return Err("Image HTTP header is invalid".into());
        }
        url_request.addValue_forHTTPHeaderField(&NSString::from_str(value), &NSString::from_str(name));
    }
    let follow_redirects = request.follow_redirects
        && request.headers.iter().all(|(name, _)| name.eq_ignore_ascii_case("if-none-match") || name.eq_ignore_ascii_case("cache-control"));
    let (completed, receiver) = mpsc::channel();
    let delegate = TransferDelegate::new(DelegateState {
        transfer: Mutex::default(),
        completed: completed.clone(),
        max_bytes: request.max_bytes,
        follow_redirects,
        progress: request.progress.clone(),
    });
    let task = session().dataTaskWithRequest(&url_request);
    task.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let cancel_task = task.clone();
    let _registration = request.on_cancel(move || {
        autoreleasepool(|_| cancel_task.cancel());
        let _ = completed.send(Err(CANCELLED.into()));
    });
    let registered_flag = request.cancellation.flag();
    if cancel.load(Ordering::Acquire) || registered_flag.load(Ordering::Acquire) {
        task.cancel();
        return Err(CANCELLED.into());
    }
    task.resume();
    let result = if ptr::eq(cancel, registered_flag) {
        receiver.recv().unwrap_or_else(|_| Err(REQUEST_FAILED.into()))
    } else {
        // An independent atomic flag cannot notify the worker, so check it while waiting.
        wait_for_atomic_cancel(&receiver, cancel)
    };
    if cancel.load(Ordering::Acquire) || registered_flag.load(Ordering::Acquire) {
        task.cancel();
        return Err(CANCELLED.into());
    }
    result
}

fn session() -> &'static NSURLSession {
    // Retain connection pools and HTTP/3 service discovery across transfers.
    static SESSION: OnceLock<Retained<NSURLSession>> = OnceLock::new();
    SESSION.get_or_init(|| {
        let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
        // The request timer covers connection setup and resets when data arrives.
        config.setTimeoutIntervalForRequest(REQUEST_IDLE_SECONDS);
        config.setTimeoutIntervalForResource(RESOURCE_SECONDS);
        config.setURLCache(None);
        config.setHTTPCookieStorage(None);
        config.setHTTPShouldSetCookies(false);
        config.setURLCredentialStorage(None);
        NSURLSession::sessionWithConfiguration(&config)
    })
}

fn wait_for_atomic_cancel(receiver: &mpsc::Receiver<FetchResult>, cancel: &AtomicBool) -> FetchResult {
    while !cancel.load(Ordering::Acquire) {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(REQUEST_FAILED.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    Err(CANCELLED.into())
}

fn is_public_http(url: &NSURL) -> bool {
    url.scheme().is_some_and(|scheme| {
        let scheme = scheme.to_string();
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    }) && url.host().is_some_and(|host| !host.is_empty())
        && url.user().is_none()
        && url.password().is_none()
}

fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        && value.bytes().all(|byte| byte == b'\t' || (byte >= b' ' && byte != 0x7f))
}

#[derive(Default)]
struct Download {
    response: Option<MediaFetchResponse>,
    bytes: Vec<u8>,
    error: Option<&'static str>,
    redirects: usize,
    probed: bool,
}

struct DelegateState {
    transfer: Mutex<Download>,
    completed: mpsc::Sender<FetchResult>,
    max_bytes: usize,
    follow_redirects: bool,
    progress: MediaProgress,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "OctaraMediaTransferDelegate"]
    #[ivars = DelegateState]
    struct TransferDelegate;

    unsafe impl NSObjectProtocol for TransferDelegate {}
    unsafe impl NSURLSessionDelegate for TransferDelegate {}

    unsafe impl NSURLSessionTaskDelegate for TransferDelegate {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        unsafe fn redirect(
            &self,
            _session: &NSURLSession,
            task: &NSURLSessionTask,
            response: &NSHTTPURLResponse,
            request: &NSURLRequest,
            complete: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            let allowed = {
                let mut transfer = self.ivars().transfer.lock().expect("media transfer");
                if let Err(error) = self.check_redirect(&transfer, response, request) {
                    transfer.error = Some(error);
                    false
                } else {
                    transfer.redirects += 1;
                    true
                }
            };
            if allowed {
                // SAFETY: Copying an NSURLRequest creates an owned NSMutableURLRequest.
                let redirected: Retained<NSMutableURLRequest> = unsafe { msg_send![request, mutableCopy] };
                redirected.setValue_forHTTPHeaderField(None, ns_string!("If-None-Match"));
                complete.call((ptr::from_ref::<NSURLRequest>(&redirected).cast_mut(),));
            } else {
                complete.call((ptr::null_mut(),));
                task.cancel();
            }
        }

        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn complete(&self, _session: &NSURLSession, _task: &NSURLSessionTask, error: Option<&NSError>) {
            let result = {
                let mut transfer = self.ivars().transfer.lock().expect("media transfer");
                if let Some(error) = transfer.error {
                    Err(error.into())
                } else if error.is_some() {
                    Err(REQUEST_FAILED.into())
                } else if let Some(mut response) = transfer.response.take() {
                    if transfer.redirects != 0 && response.status == 304 {
                        Err("Image redirect returned an unsolicited 304".into())
                    } else {
                        if transfer.redirects != 0 {
                            // Validators belong to the effective resource, not the original redirect URL.
                            response.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("etag"));
                        }
                        response.bytes = std::mem::take(&mut transfer.bytes).into();
                        Ok(response)
                    }
                } else {
                    Err(REQUEST_FAILED.into())
                }
            };
            let _ = self.ivars().completed.send(result);
        }
    }

    unsafe impl NSURLSessionDataDelegate for TransferDelegate {
        #[unsafe(method(URLSession:dataTask:didReceiveResponse:completionHandler:))]
        unsafe fn receive_response(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionDataTask,
            response: &NSURLResponse,
            complete: &DynBlock<dyn Fn(NSURLSessionResponseDisposition)>,
        ) {
            let disposition = {
                let mut transfer = self.ivars().transfer.lock().expect("media transfer");
                match (transfer.error, response_metadata(response, self.ivars().max_bytes)) {
                    (None, Ok(response)) => {
                        transfer.response = Some(response);
                        NSURLSessionResponseDisposition::Allow
                    }
                    (None, Err(error)) => {
                        transfer.error = Some(error);
                        NSURLSessionResponseDisposition::Cancel
                    }
                    (Some(_), _) => NSURLSessionResponseDisposition::Cancel,
                }
            };
            complete.call((disposition,));
        }

        #[unsafe(method(URLSession:dataTask:didReceiveData:))]
        fn receive_data(&self, _session: &NSURLSession, task: &NSURLSessionDataTask, data: &NSData) {
            let mut transfer = self.ivars().transfer.lock().expect("media transfer");
            if transfer.error.is_some() {
                return;
            }
            let max_bytes = self.ivars().max_bytes;
            let received = transfer.bytes.len();
            if data.len() > max_bytes - received {
                transfer.error = Some(RESPONSE_TOO_LARGE);
                drop(transfer);
                task.cancel();
                return;
            }
            let new_len = received + data.len();
            let capacity = transfer.bytes.capacity();
            if new_len > capacity {
                let target = capacity.saturating_mul(2).max(new_len).min(max_bytes);
                transfer.bytes.reserve_exact(target - received);
            }
            // SAFETY: URLSession delivers immutable data for this delegate call.
            transfer.bytes.extend_from_slice(unsafe { data.as_bytes_unchecked() });
            let size = if !transfer.probed && received < PROBE_LIMIT { probe_image_size(&transfer.bytes) } else { None };
            if let Some(size) = size {
                transfer.probed = true;
                drop(transfer);
                self.ivars().progress.probed(size);
            }
        }
    }
);

impl TransferDelegate {
    fn new(state: DelegateState) -> Retained<Self> {
        // SAFETY: The direct superclass is NSObject and has no initialization arguments.
        unsafe { msg_send![super(Self::alloc().set_ivars(state)), init] }
    }

    fn check_redirect(&self, transfer: &Download, response: &NSHTTPURLResponse, request: &NSURLRequest) -> Result<(), &'static str> {
        if !self.ivars().follow_redirects {
            return Err("Image HTTP redirects are disabled for this request");
        }
        if transfer.redirects >= REDIRECT_LIMIT {
            return Err("Image HTTP redirect limit exceeded");
        }
        let destination = response.valueForHTTPHeaderField(ns_string!("Location")).and_then(|location| {
            NSURL::URLWithString_relativeToURL(&location, response.URL().as_deref()).and_then(|url| url.absoluteURL())
        });
        if !destination.as_deref().is_some_and(is_public_http) || !request.URL().as_deref().is_some_and(is_public_http) {
            return Err("Image HTTP redirect must use HTTP or HTTPS without URL credentials");
        }
        Ok(())
    }
}

fn response_metadata(response: &NSURLResponse, max_bytes: usize) -> Result<MediaFetchResponse, &'static str> {
    let response = response.downcast_ref::<NSHTTPURLResponse>().ok_or(REQUEST_FAILED)?;
    let status = u16::try_from(response.statusCode()).map_err(|_| REQUEST_FAILED)?;
    // A 304 can specify the stored representation's length while carrying no body.
    if status != 304 && usize::try_from(response.expectedContentLength()).is_ok_and(|length| length > max_bytes) {
        return Err(RESPONSE_TOO_LARGE);
    }
    let (keys, values) = response.allHeaderFields().to_vecs();
    let headers = keys
        .into_iter()
        .zip(values)
        .filter_map(|(name, value)| {
            let name = name.downcast::<NSString>().ok()?;
            let value = value.downcast::<NSString>().ok()?;
            Some((name.to_string().to_ascii_lowercase().into(), value.to_string().into()))
        })
        .collect();
    Ok(MediaFetchResponse { status, headers, bytes: Arc::from([]) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Instant;

    fn listen() -> TcpListener {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    fn serve(handler: impl FnOnce(TcpListener) + Send + 'static) -> (String, thread::JoinHandle<()>) {
        let listener = listen();
        let uri = format!("http://{}/image", listener.local_addr().unwrap());
        (uri, thread::spawn(move || handler(listener)))
    }

    fn receive(listener: &TcpListener) -> (TcpStream, String) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("Could not accept image request: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut request = String::new();
        loop {
            let offset = request.len();
            assert_ne!(reader.read_line(&mut request).unwrap(), 0);
            if &request[offset..] == "\r\n" {
                return (stream, request);
            }
        }
    }

    fn fetch(request: MediaFetchRequest) -> FetchResult {
        let cancellation = request.cancellation.clone();
        AppleMediaTransport.fetch(request, cancellation.flag())
    }

    fn header<'a>(response: &'a MediaFetchResponse, key: &str) -> Option<&'a str> {
        response.headers.iter().find(|(name, _)| name.as_ref() == key).map(|(_, value)| value.as_ref())
    }

    fn expect_disconnect(stream: &mut TcpStream) {
        let closed = stream.read(&mut [0]);
        assert!(matches!(closed, Ok(0)) || closed.is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset));
    }

    #[test]
    fn returns_etag_and_passes_through_304() {
        let (uri, server) = serve(|listener| {
            let (mut stream, request) = receive(&listener);
            assert!(request.starts_with("GET /image HTTP/"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nETag: \"v1\"\r\nConnection: close\r\n\r\ndata").unwrap();
            drop(stream);
            let (mut stream, request) = receive(&listener);
            assert!(request.to_ascii_lowercase().contains("\r\nif-none-match: \"v1\"\r\n"));
            stream.write_all(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 4\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n").unwrap();
        });
        let response = fetch(MediaFetchRequest::new(uri.as_str(), 4)).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(&*response.bytes, b"data");
        assert_eq!(header(&response, "etag"), Some("\"v1\""));

        let mut request = MediaFetchRequest::new(uri, 0);
        request.headers.push(("If-None-Match".into(), "\"v1\"".into()));
        let response = fetch(request).unwrap();
        assert_eq!(response.status, 304);
        assert!(response.bytes.is_empty());
        assert_eq!(header(&response, "etag"), Some("\"v1\""));
        server.join().unwrap();
    }

    #[test]
    fn follows_relative_redirects_and_enforces_the_hop_limit() {
        let (uri, server) = serve(|listener| {
            let (mut stream, initial) = receive(&listener);
            assert!(initial.to_ascii_lowercase().contains("\r\nif-none-match: \"v1\"\r\n"));
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /new\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(stream);
            let (mut stream, request) = receive(&listener);
            assert!(request.starts_with("GET /new HTTP/"));
            assert!(!request.to_ascii_lowercase().contains("\r\nif-none-match:"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nok").unwrap();
        });
        let mut request = MediaFetchRequest::new(uri, 2);
        request.follow_redirects = true;
        request.headers.push(("If-None-Match".into(), "\"v1\"".into()));
        let response = fetch(request).unwrap();
        assert_eq!(&*response.bytes, b"ok");
        assert!(header(&response, "etag").is_none());
        server.join().unwrap();

        let (uri, server) = serve(|listener| {
            for index in 0..=REDIRECT_LIMIT {
                let (mut stream, _) = receive(&listener);
                write!(stream, "HTTP/1.1 302 Found\r\nLocation: /hop{index}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        });
        let mut request = MediaFetchRequest::new(uri, 2);
        request.follow_redirects = true;
        assert_eq!(&*fetch(request).err().unwrap(), "Image HTTP redirect limit exceeded");
        server.join().unwrap();
    }

    #[test]
    fn rejects_initial_url_credentials_and_unsafe_redirects() {
        let destination = listen();
        let addr = destination.local_addr().unwrap();
        for uri in [format!("http://user:pass@{addr}/image"), format!("http://@{addr}/image"), "file:///image".into()] {
            assert!(fetch(MediaFetchRequest::new(uri, 8)).is_err());
        }
        for (location, header) in [
            (format!("http://user:pass@{addr}/image"), None),
            ("file:///image".into(), None),
            (format!("http://{addr}/image"), Some(("X-Private", "value"))),
            (format!("http://{addr}/image"), Some(("Authorization", "Bearer token"))),
        ] {
            let (uri, server) = serve(move |listener| {
                let (mut stream, request) = receive(&listener);
                if let Some((name, value)) = header {
                    assert!(request.to_ascii_lowercase().contains(&format!("\r\n{name}: {value}\r\n").to_ascii_lowercase()));
                }
                write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let mut request = MediaFetchRequest::new(uri, 8);
            request.follow_redirects = true;
            if let Some((name, value)) = header {
                request.headers.push((name.into(), value.into()));
            }
            assert!(fetch(request).is_err());
            server.join().unwrap();
        }
        assert_eq!(destination.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn cancels_oversized_responses_before_end_of_stream() {
        let max_bytes = 48 * 1024;
        let prefix = "x".repeat(8 * 1024);
        let known =
            format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{prefix}", max_bytes + 1);
        let chunk = format!("8000\r\n{}\r\n", "x".repeat(32 * 1024));
        let chunked =
            format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{chunk}{chunk}");
        for (case, response) in [("Content-Length", known), ("chunked", chunked)] {
            let (uri, server) = serve(move |listener| {
                let (mut stream, _) = receive(&listener);
                stream.write_all(response.as_bytes()).unwrap();
                expect_disconnect(&mut stream);
            });
            assert_eq!(&*fetch(MediaFetchRequest::new(uri, max_bytes)).err().unwrap(), RESPONSE_TOO_LARGE, "{case}");
            server.join().unwrap();
        }
    }

    #[test]
    fn cancels_native_task_while_waiting_for_headers() {
        let (started, server_started) = mpsc::channel();
        let (uri, server) = serve(move |listener| {
            let (mut stream, _) = receive(&listener);
            started.send(()).unwrap();
            expect_disconnect(&mut stream);
        });
        let request = MediaFetchRequest::new(uri, 8);
        let cancellation = request.cancellation.clone();
        let (done, completed) = mpsc::channel();
        let worker = thread::spawn(move || done.send(fetch(request)).unwrap());
        server_started.recv_timeout(Duration::from_secs(5)).unwrap();
        cancellation.cancel();
        let error = completed.recv_timeout(Duration::from_secs(2)).unwrap().err().unwrap();
        assert_eq!(&*error, CANCELLED);
        server.join().unwrap();
        worker.join().unwrap();
    }
}
