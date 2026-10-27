use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE,
};
use reqwest::{Client, Response, StatusCode, Url};
use url::Host;

use super::{DownloadError, MAX_REDIRECTS, ObjectSpec};

pub(super) async fn send_with_redirects(
    client: &Client,
    initial_url: &Url,
    range_start: Option<u64>,
) -> Result<Response, DownloadError> {
    let mut url = initial_url.clone();
    let mut require_https = url.scheme() == "https";

    for redirect_count in 0..=MAX_REDIRECTS {
        let mut request = client.get(url.clone()).header(ACCEPT_ENCODING, "identity");
        if let Some(start) = range_start {
            request = request.header(RANGE, format!("bytes={start}-"));
        }
        let response = request.send().await.map_err(DownloadError::Transport)?;
        if !is_redirect_status(response.status()) {
            return Ok(response);
        }
        if redirect_count == MAX_REDIRECTS {
            return Err(DownloadError::InvalidRedirect);
        }

        let locations = response.headers().get_all(LOCATION);
        let mut values = locations.iter();
        let location = values
            .next()
            .and_then(|value| value.to_str().ok())
            .ok_or(DownloadError::InvalidRedirect)?;
        if values.next().is_some() {
            return Err(DownloadError::InvalidRedirect);
        }
        let next = url
            .join(location)
            .map_err(|_| DownloadError::InvalidRedirect)?;
        validate_url(&next, require_https).map_err(|_| DownloadError::InvalidRedirect)?;
        require_https |= next.scheme() == "https";
        url = next;
    }

    Err(DownloadError::InvalidRedirect)
}

pub(super) fn object_url(root: &str, object_key: &str) -> Result<Url, DownloadError> {
    let mut url = Url::parse(root).map_err(|_| DownloadError::InvalidRoot)?;
    validate_url(&url, false)?;

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| DownloadError::InvalidRoot)?;
        segments.pop_if_empty();
        for segment in object_key.split('/') {
            segments.push(segment);
        }
    }
    Ok(url)
}

fn validate_object_key(object_key: &str) -> Result<(), DownloadError> {
    if object_key.is_empty()
        || object_key.starts_with('/')
        || object_key.contains('\\')
        || object_key.chars().any(char::is_control)
        || object_key
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(DownloadError::InvalidObjectKey);
    }
    Ok(())
}

fn validate_url(url: &Url, require_https: bool) -> Result<(), DownloadError> {
    if has_user_info(url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DownloadError::InvalidRoot);
    }

    match url.scheme() {
        "https" => Ok(()),
        "http" if !require_https && is_loopback_host(url) => Ok(()),
        _ => Err(DownloadError::InvalidRoot),
    }
}

fn has_user_info(url: &Url) -> bool {
    url.as_str()
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

pub(super) fn validate_spec(spec: &ObjectSpec) -> Result<String, DownloadError> {
    validate_object_key(&spec.object_key)?;
    if spec.sha256.len() != 64 || !spec.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DownloadError::InvalidSpec);
    }
    Ok(spec.sha256.to_ascii_lowercase())
}

pub(super) fn parse_content_range(response: &Response) -> Result<ByteRange, DownloadError> {
    let values = response.headers().get_all(CONTENT_RANGE);
    let mut headers = values.iter();
    let value = headers
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(DownloadError::ContentRange)?;
    if headers.next().is_some() {
        return Err(DownloadError::ContentRange);
    }
    let value = value
        .strip_prefix("bytes ")
        .ok_or(DownloadError::ContentRange)?;
    let (range, total) = value.split_once('/').ok_or(DownloadError::ContentRange)?;
    let (start, end) = range.split_once('-').ok_or(DownloadError::ContentRange)?;
    let start = parse_decimal(start).ok_or(DownloadError::ContentRange)?;
    let end = parse_decimal(end).ok_or(DownloadError::ContentRange)?;
    let total = parse_decimal(total).ok_or(DownloadError::ContentRange)?;
    Ok(ByteRange { start, end, total })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ByteRange {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) total: u64,
}

fn parse_decimal(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

pub(super) fn validate_representation_headers(
    response: &Response,
    expected_body_bytes: u64,
) -> Result<(), DownloadError> {
    let encodings = response.headers().get_all(CONTENT_ENCODING);
    let mut values = encodings.iter();
    if let Some(value) = values.next()
        && (values.next().is_some()
            || !value
                .to_str()
                .map(|value| value.eq_ignore_ascii_case("identity"))
                .unwrap_or(false))
    {
        return Err(DownloadError::ContentEncoding);
    }

    let content_types = response.headers().get_all(CONTENT_TYPE);
    let mut values = content_types.iter();
    if let Some(value) = values.next() {
        if values.next().is_some() {
            return Err(DownloadError::HtmlResponse);
        }
        let media_type = value
            .to_str()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        if media_type.eq_ignore_ascii_case("text/html")
            || media_type.eq_ignore_ascii_case("application/xhtml+xml")
        {
            return Err(DownloadError::HtmlResponse);
        }
    }

    let lengths = response.headers().get_all(CONTENT_LENGTH);
    let mut values = lengths.iter();
    if let Some(value) = values.next() {
        if values.next().is_some() {
            return Err(DownloadError::ResponseLength);
        }
        let value = value
            .to_str()
            .ok()
            .and_then(parse_decimal)
            .ok_or(DownloadError::ResponseLength)?;
        if value != expected_body_bytes {
            return Err(DownloadError::ResponseLength);
        }
    }
    Ok(())
}

pub(super) fn has_header(
    response: &Response,
    name: &reqwest::header::HeaderName,
) -> Result<bool, DownloadError> {
    let values = response.headers().get_all(name);
    let mut headers = values.iter();
    let has_one = headers.next().is_some();
    if headers.next().is_some() {
        return Err(DownloadError::ResponseLength);
    }
    Ok(has_one)
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

pub(super) fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}
