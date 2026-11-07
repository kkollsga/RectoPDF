//! An ordinary download, by the system's own `curl`: nothing of a TLS stack or an HTTP client stays in the app, and when it's done nothing's left. No cookies, no credentials, the user's `curlrc` skipped, and redirects only to where a release can really go. The server's hostile: what it sends is capped, how long it takes is capped, and nothing it says gets into a message.

use crate::process::run;
use crate::source::Source;
use sha2::Digest as _;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const CURL: &str = "/usr/bin/curl";
/// A check gives up after this.
pub const CHECK_LIMIT: Duration = Duration::from_secs(15);
/// A download gives up after this.
pub const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);
/// The most a release's zip can be. Far more than the app.
pub const MAX_ZIP_BYTES: u64 = 512 * 1024 * 1024;
/// Time past `curl`'s own limit before it's stopped from outside.
const OUTSIDE_MARGIN: Duration = Duration::from_secs(5);

/// Why a fetch failed. Words of our own: nothing a server said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    TimedOut,
    TooLarge,
    NotFound,
    Status(u16),
    Failed(&'static str),
}

impl FetchError {
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::TimedOut => "it took too long".to_owned(),
            Self::TooLarge => "the server sent too much".to_owned(),
            Self::NotFound => "there's no such release".to_owned(),
            Self::Status(status) => format!("the server answered {status}"),
            Self::Failed(words) => (*words).to_owned(),
        }
    }
}

fn arguments(source: &Source, url: &str, user_agent: &str, limit: Duration) -> Vec<String> {
    let protocol = if source.allows_plain_http() { "=http,https" } else { "=https" };
    let mut args: Vec<String> = ["-q", "--silent", "--show-error", "--location", "--max-redirs", "5", "--proto", protocol, "--proto-redir", protocol, "--connect-timeout", "10", "--write-out", "%{stderr}\nstatus=%{http_code}\n"].map(str::to_owned).to_vec();
    args.extend(["--user-agent".to_owned(), user_agent.to_owned()]);
    args.extend(["--max-time".to_owned(), format!("{:.3}", limit.as_secs_f64())]);
    args.extend(["--speed-limit".to_owned(), "1".to_owned(), "--speed-time".to_owned(), "60".to_owned()]);
    args.extend(["--output".to_owned(), "-".to_owned()]);
    args.push("--".to_owned());
    args.push(url.to_owned());
    args
}

fn command(source: &Source, url: &str, user_agent: &str, limit: Duration) -> Command {
    let mut command = Command::new(CURL);
    command.args(arguments(source, url, user_agent, limit));
    // A GUI app has none of a shell's variables to begin with, and this way a test run and a real one do the same.
    command.env_clear();
    command
}

/// What `curl`'s exit code comes to.
fn curl_failure(code: Option<i32>) -> FetchError {
    match code {
        Some(28) => FetchError::TimedOut,
        Some(63) => FetchError::TooLarge,
        Some(5 | 6) => FetchError::Failed("the server couldn't be found"),
        Some(7) => FetchError::Failed("the server couldn't be reached"),
        Some(35 | 60 | 77) => FetchError::Failed("a secure connection couldn't be made"),
        Some(1) => FetchError::Failed("it was sent somewhere an update can't come from"),
        Some(47) => FetchError::Failed("too many redirects"),
        _ => FetchError::Failed("a network error"),
    }
}

/// The status it wrote last, after whatever else it said.
fn status_from(stderr: &[u8]) -> Option<u16> {
    let text = String::from_utf8_lossy(stderr);
    text.lines().rev().find_map(|line| line.strip_prefix("status=")).and_then(|status| status.trim().parse().ok())
}

/// A small GET: a checked status, and what came, up to `max_bytes`.
pub fn get_small(source: &Source, url: &str, user_agent: &str, max_bytes: usize, limit: Duration) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    let mut over = false;
    let ran = run(command(source, url, user_agent, limit), limit + OUTSIDE_MARGIN, |bytes| {
        if body.len() + bytes.len() > max_bytes {
            over = true;
            return false;
        }
        body.extend_from_slice(bytes);
        true
    })
    .map_err(|_| FetchError::Failed("the download tool couldn't be started"))?;
    if over {
        return Err(FetchError::TooLarge);
    }
    if ran.timed_out {
        return Err(FetchError::TimedOut);
    }
    if ran.code != Some(0) {
        return Err(curl_failure(ran.code));
    }
    match status_from(&ran.stderr) {
        Some(200) => Ok(body),
        Some(404) => Err(FetchError::NotFound),
        Some(status) => Err(FetchError::Status(status)),
        None => Err(FetchError::Failed("a network error")),
    }
}

/// Downloads to a file, capped, and says its SHA-256. The file's removed unless this comes back with one.
pub fn download(source: &Source, url: &str, user_agent: &str, to: &Path, max_bytes: u64, limit: Duration) -> Result<String, FetchError> {
    let cant = || FetchError::Failed("the download couldn't be written");
    let _ = std::fs::remove_file(to);
    let mut file = std::fs::File::create(to).map_err(|_| cant())?;
    let mut hash = sha2::Sha256::new();
    let mut total = 0u64;
    let mut over = false;
    let mut broke = false;
    let result = run(command(source, url, user_agent, limit), limit + OUTSIDE_MARGIN, |bytes| {
        total += bytes.len() as u64;
        if total > max_bytes {
            over = true;
            return false;
        }
        hash.update(bytes);
        if file.write_all(bytes).is_err() {
            broke = true;
            return false;
        }
        true
    });
    let outcome = (|| {
        let ran = result.map_err(|_| FetchError::Failed("the download tool couldn't be started"))?;
        if over {
            return Err(FetchError::TooLarge);
        }
        if broke {
            return Err(cant());
        }
        if ran.timed_out {
            return Err(FetchError::TimedOut);
        }
        if ran.code != Some(0) {
            return Err(curl_failure(ran.code));
        }
        match status_from(&ran.stderr) {
            Some(200) => {}
            Some(404) => return Err(FetchError::NotFound),
            Some(status) => return Err(FetchError::Status(status)),
            None => return Err(FetchError::Failed("a network error")),
        }
        file.flush().map_err(|_| cant())?;
        Ok(hash.clone().finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(to);
    }
    outcome
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod tests {
    use super::*;
    use crate::source::resolve;
    use std::io::Read as _;
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    /// What a test's server does with a request for a path.
    #[derive(Clone)]
    pub(crate) enum Answer {
        Body(Vec<u8>),
        Status(u16),
        /// Takes the request and never says anything.
        Hang,
        /// Sends without end.
        Forever,
        /// Starts a body it says is long, and stops.
        Cut,
        Redirect(String),
    }

    /// A small server on this machine, as hostile as a test asks it to be.
    pub(crate) struct Server {
        pub url: String,
        pub requests: Arc<Mutex<Vec<String>>>,
        answers: Arc<Mutex<Vec<(String, Answer)>>>,
    }

    impl Server {
        pub(crate) fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let answers: Arc<Mutex<Vec<(String, Answer)>>> = Arc::new(Mutex::new(Vec::new()));
            let (log, table) = (Arc::clone(&requests), Arc::clone(&answers));
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (log, table) = (Arc::clone(&log), Arc::clone(&table));
                    std::thread::spawn(move || answer_one(stream, &log, &table));
                }
            });
            Self { url, requests, answers }
        }

        pub(crate) fn set(&self, path: &str, answer: Answer) {
            let mut answers = self.answers.lock().unwrap();
            answers.retain(|(known, _)| known != path);
            answers.push((path.to_owned(), answer));
        }

        pub(crate) fn source(&self) -> Source {
            resolve(false, true, Some(self.url.clone()), None)
        }
    }

    fn answer_one(mut stream: TcpStream, log: &Mutex<Vec<String>>, table: &Mutex<Vec<(String, Answer)>>) {
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => request.extend_from_slice(&buffer[..count]),
            }
        }
        let text = String::from_utf8_lossy(&request).to_string();
        log.lock().unwrap().push(text.clone());
        let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
        let found = table.lock().unwrap().iter().find(|(known, _)| *known == path).map(|(_, answer)| answer.clone());
        match found.unwrap_or(Answer::Status(404)) {
            Answer::Body(body) => {
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
            Answer::Status(status) => {
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
            Answer::Hang => {
                std::thread::sleep(Duration::from_secs(120));
            }
            Answer::Forever => {
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let chunk = vec![b'x'; 64 * 1024];
                while stream.write_all(&chunk).is_ok() {}
            }
            Answer::Cut => {
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\nshort");
            }
            Answer::Redirect(to) => {
                let _ = write!(stream, "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        }
    }

    const LIMIT: Duration = Duration::from_millis(1500);

    /// Failure: a body doesn't arrive whole, or a 404 reads as a failure of the network rather than no release.
    #[test]
    fn a_small_get_says_what_came() {
        let server = Server::start();
        server.set("/a", Answer::Body(b"hello".to_vec()));
        let source = server.source();
        let url = format!("{}/a", server.url);
        assert_eq!(get_small(&source, &url, "RectoPDF/0.1.0", 1024, LIMIT).unwrap(), b"hello");
        assert_eq!(get_small(&source, &format!("{}/none", server.url), "x", 1024, LIMIT), Err(FetchError::NotFound));
        server.set("/b", Answer::Status(500));
        assert_eq!(get_small(&source, &format!("{}/b", server.url), "x", 1024, LIMIT), Err(FetchError::Status(500)));
    }

    /// Failure: what goes to the server carries anything but an ordinary download request: a document's name, a cookie, or who the user is. The owner's rule: "an ordinary download request with no document data".
    #[test]
    fn a_check_sends_nothing_of_the_user() {
        let server = Server::start();
        server.set("/a", Answer::Body(b"x".to_vec()));
        get_small(&server.source(), &format!("{}/a", server.url), "RectoPDF/0.1.0", 1024, LIMIT).unwrap();
        let requests = server.requests.lock().unwrap();
        let request = requests[0].to_ascii_lowercase();
        assert!(request.contains("user-agent: rectopdf/0.1.0"));
        for private in ["cookie", "authorization", "referer", ".pdf"] {
            assert!(!request.contains(private), "{request}");
        }
        assert_eq!(request.lines().filter(|line| !line.is_empty()).count(), 4, "get, host, user agent, accept: {request}");
    }

    /// Failure: a server that takes the request and never answers holds a thread past the limit.
    #[test]
    fn a_server_that_hangs_times_out() {
        let server = Server::start();
        server.set("/a", Answer::Hang);
        let started = std::time::Instant::now();
        assert_eq!(get_small(&server.source(), &format!("{}/a", server.url), "x", 1024, LIMIT), Err(FetchError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    }

    /// Failure: a server that sends for ever fills memory or the disk, in a check or a download.
    #[test]
    fn a_server_that_sends_for_ever_is_stopped() {
        let server = Server::start();
        server.set("/a", Answer::Forever);
        let source = server.source();
        let url = format!("{}/a", server.url);
        assert_eq!(get_small(&source, &url, "x", 64 * 1024, Duration::from_secs(20)), Err(FetchError::TooLarge));
        let file = std::env::temp_dir().join(format!("recto-fetch-{}-forever", std::process::id()));
        assert_eq!(download(&source, &url, "x", &file, 1024 * 1024, Duration::from_secs(20)), Err(FetchError::TooLarge));
        assert!(!file.exists(), "nothing's left behind");
    }

    /// Failure: a download cut short, or answered with an error page, counts as a good file.
    #[test]
    fn a_download_is_whole_or_it_is_nothing() {
        let server = Server::start();
        let source = server.source();
        let file = std::env::temp_dir().join(format!("recto-fetch-{}-whole", std::process::id()));
        server.set("/ok", Answer::Body(b"abc".to_vec()));
        let sha = download(&source, &format!("{}/ok", server.url), "x", &file, 1024, LIMIT).unwrap();
        assert_eq!(sha, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(std::fs::read(&file).unwrap(), b"abc");
        server.set("/cut", Answer::Cut);
        assert!(download(&source, &format!("{}/cut", server.url), "x", &file, 1024 * 1024, LIMIT).is_err());
        assert!(!file.exists());
        server.set("/err", Answer::Status(503));
        assert_eq!(download(&source, &format!("{}/err", server.url), "x", &file, 1024, LIMIT), Err(FetchError::Status(503)));
        assert!(!file.exists());
    }

    /// Failure: a redirect takes a release build to plain HTTP, or to a local file, so a bad answer could get a download from anywhere.
    #[test]
    fn redirects_cant_leave_https_in_a_real_build() {
        let real = resolve(true, false, None, None);
        let args = arguments(&real, "https://example.invalid/x", "x", LIMIT);
        let at = args.iter().position(|arg| arg == "--proto-redir").unwrap();
        assert_eq!(args[at + 1], "=https");
        let at = args.iter().position(|arg| arg == "--proto").unwrap();
        assert_eq!(args[at + 1], "=https");
        assert_eq!(args[0], "-q", "the user's curlrc is skipped, and only as the first word does that work");
        // And a test's own server can redirect only by the same rule.
        let server = Server::start();
        server.set("/a", Answer::Redirect("file:///etc/passwd".into()));
        assert!(get_small(&server.source(), &format!("{}/a", server.url), "x", 1024, LIMIT).is_err());
    }
}
