//! A small WinHTTP client. WinHTTP signs in with the current Windows login (Negotiate)
//! without a prompt, which is the point.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

const CLIENT: &str = concat!("claude-comptroller-connect/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub struct Response {
    pub status: u32,
    pub body: String,
}

/// The server's base URL, split for WinHTTP.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub secure: bool,
    pub host: String,
    pub port: u16,
}

impl Server {
    /// Accepts `http(s)://host[:port]` with an optional trailing slash.
    pub fn parse(url: &str) -> Result<Self, String> {
        let invalid = || format!("\"{url}\" isn't a server address like https://comptroller.example.internal.");
        let url = url.trim().trim_end_matches('/');
        let (scheme, rest) = url.split_once("://").ok_or_else(invalid)?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(invalid()),
        };
        if rest.is_empty() || rest.contains(['/', '@', '?', '#', ' ']) {
            return Err(invalid());
        }
        let default = if secure { 443 } else { 80 };
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (host, port.parse().map_err(|_| invalid())?),
            _ => (rest, default),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return Err(invalid());
        }
        Ok(Self { secure, host: host.to_owned(), port })
    }

    pub fn url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let default = if self.secure { 443 } else { 80 };
        if self.port == default { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{}", self.port) }
    }

    /// Plain HTTP to another machine would send the token readable by anyone on the path.
    pub fn is_exposed(&self) -> bool {
        !self.secure && !matches!(self.host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
    }
}

struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() { Err(windows::core::Error::from_thread().message()) } else { Ok(Self(raw)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a str>,
    /// Answer a sign-in challenge with the current Windows login.
    pub windows_auth: bool,
}

/// An `Err` means the server couldn't be reached.
pub fn send(server: &Server, request: &Request) -> Result<Response, String> {
    let network = |e: windows::core::Error| e.message();

    let mut headers = format!("X-Comptroller-Client: {CLIENT}\r\n");
    if request.body.is_some() {
        headers.push_str("Content-Type: application/json\r\n");
    }
    for (name, value) in request.headers {
        if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err("A header contains a line break.".into());
        }
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    let headers: Vec<u16> = headers.encode_utf16().collect();
    let body = request.body.unwrap_or("").as_bytes();

    // SAFETY: every pointer passed to WinHTTP outlives the call it's passed to.
    unsafe {
        // The server is on the company network: no web proxy.
        let session = Handle::new(WinHttpOpen(
            &HSTRING::from(CLIENT), WINHTTP_ACCESS_TYPE_NO_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(session.0, 5_000, 5_000, 10_000, 15_000).map_err(network)?;
        let connection = Handle::new(WinHttpConnect(session.0, &HSTRING::from(server.host.as_str()), server.port, 0))?;
        let flags = if server.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let handle = Handle::new(WinHttpOpenRequest(
            connection.0, &HSTRING::from(request.method), &HSTRING::from(request.path),
            PCWSTR::null(), PCWSTR::null(), null(), flags))?;

        // Credentials must never follow a redirect to another host.
        set_option(&handle, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER).map_err(network)?;
        if request.windows_auth {
            // The default only signs in to "intranet zone" names, which excludes fully
            // qualified ones. This client only ever talks to the configured server.
            set_option(&handle, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW).map_err(network)?;
        }

        let mut signed_in = false;
        loop {
            WinHttpSendRequest(
                handle.0, Some(&headers), (!body.is_empty()).then_some(body.as_ptr().cast()),
                body.len() as u32, body.len() as u32, 0)
                .map_err(network)?;
            WinHttpReceiveResponse(handle.0, null_mut()).map_err(network)?;
            let status = status_of(&handle).map_err(network)?;

            if status == 401 && request.windows_auth && !signed_in {
                let (mut supported, mut first, mut target) = (0, 0, 0);
                if WinHttpQueryAuthSchemes(handle.0, &mut supported, &mut first, &mut target).is_ok()
                    && supported & WINHTTP_AUTH_SCHEME_NEGOTIATE.0 != 0
                {
                    // No user name or password: use the current login.
                    WinHttpSetCredentials(
                        handle.0, WINHTTP_AUTH_TARGET_SERVER, WINHTTP_AUTH_SCHEME_NEGOTIATE.0,
                        PCWSTR::null(), PCWSTR::null(), null_mut())
                        .map_err(network)?;
                    signed_in = true;
                    // Drain the challenge so the connection can be reused for the handshake.
                    read_body(&handle).map_err(network)?;
                    continue;
                }
            }

            return Ok(Response { status, body: read_body(&handle).map_err(network)? });
        }
    }
}

unsafe fn set_option(handle: &Handle, option: u32, value: u32) -> windows::core::Result<()> {
    WinHttpSetOption(Some(handle.0), option, Some(&value.to_ne_bytes()))
}

unsafe fn status_of(handle: &Handle) -> windows::core::Result<u32> {
    let mut status = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    WinHttpQueryHeaders(
        handle.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(),
        Some((&mut status as *mut u32).cast()), &mut size, null_mut())?;
    Ok(status)
}

unsafe fn read_body(handle: &Handle) -> windows::core::Result<String> {
    let mut body = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let mut read = 0u32;
        WinHttpReadData(handle.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read)?;
        // Nothing the service sends is anywhere near this big.
        if read == 0 || body.len() > 1_000_000 {
            return Ok(String::from_utf8_lossy(&body).into_owned());
        }
        body.extend_from_slice(&buffer[..read as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_addresses() {
        let server = Server::parse(" https://comptroller.example.internal/ ").unwrap();
        assert_eq!((server.secure, server.port), (true, 443));
        assert_eq!(server.url(), "https://comptroller.example.internal");
        assert_eq!(Server::parse("http://localhost:8080").unwrap().url(), "http://localhost:8080");
        assert_eq!(Server::parse("http://[::1]:8080").unwrap().url(), "http://[::1]:8080");
        for bad in ["comptroller", "ftp://x", "https://", "https://a/b", "https://user@host", "https://host:abc"] {
            assert!(Server::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_remote_plain_http_is_exposed() {
        assert!(Server::parse("http://comptroller.example.internal").unwrap().is_exposed());
        assert!(!Server::parse("http://localhost:8080").unwrap().is_exposed());
        assert!(!Server::parse("https://comptroller.example.internal").unwrap().is_exposed());
    }
}
