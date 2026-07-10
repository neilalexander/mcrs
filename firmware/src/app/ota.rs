use embedded_io_async::{Read, Write};

const HEADER_BUFFER_LEN: usize = 1024;
const BODY_BUFFER_LEN: usize = 1024;
const INDEX_HTML: &[u8] = br#"<!doctype html>
<html>
<head><meta name="viewport" content="width=device-width,initial-scale=1"><title>MeshCore OTA</title></head>
<body>
<h1>MeshCore OTA</h1>
<input id="file" type="file" accept=".bin,application/octet-stream">
<button onclick="upload()">Upload</button>
<pre id="out"></pre>
<script>
async function upload() {
  const file = document.getElementById('file').files[0];
  if (!file) return;
  const out = document.getElementById('out');
  out.textContent = 'Uploading...';
  const res = await fetch('/update', { method: 'POST', headers: {'Content-Type':'application/octet-stream'}, body: file });
  out.textContent = await res.text();
}
</script>
</body>
</html>
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpOtaError {
    Io,
    BadRequest,
    Ota(crate::platform::OtaError),
}

impl From<crate::platform::OtaError> for HttpOtaError {
    fn from(error: crate::platform::OtaError) -> Self {
        Self::Ota(error)
    }
}

pub async fn handle_connection<C>(connection: &mut C) -> Result<(), HttpOtaError>
where
    C: Read + Write,
{
    let mut header = [0u8; HEADER_BUFFER_LEN];
    let (header_len, body_start) = read_header(connection, &mut header).await?;
    let request = &header[..header_len];

    if request.starts_with(b"GET / ") || request.starts_with(b"GET /HTTP/") {
        write_response(connection, "200 OK", "text/html", INDEX_HTML).await?;
        return Ok(());
    }

    if !request.starts_with(b"POST /update ") && !request.starts_with(b"POST /update?") {
        write_response(connection, "404 Not Found", "text/plain", b"Not found\n").await?;
        return Ok(());
    }

    let content_len = content_length(request).ok_or(HttpOtaError::BadRequest)?;
    match receive_update(connection, &header[body_start..header_len], content_len).await {
        Ok(bytes) => {
            let mut response = [0u8; 64];
            let len = write_decimal_response(
                &mut response,
                b"OK - wrote ",
                bytes,
                b" bytes; reboot to apply\n",
            );
            write_response(connection, "200 OK", "text/plain", &response[..len]).await?;
        }
        Err(error) => {
            let body = ota_error_body(error);
            write_response(connection, "400 Bad Request", "text/plain", body).await?;
            return Err(error);
        }
    }

    Ok(())
}

async fn read_header<C>(
    connection: &mut C,
    header: &mut [u8; HEADER_BUFFER_LEN],
) -> Result<(usize, usize), HttpOtaError>
where
    C: Read,
{
    let mut len = 0;
    loop {
        if len == header.len() {
            return Err(HttpOtaError::BadRequest);
        }
        let count = read_io(connection, &mut header[len..]).await?;
        if count == 0 {
            return Err(HttpOtaError::BadRequest);
        }
        len += count;
        if let Some(body_start) = find_header_end(&header[..len]) {
            return Ok((len, body_start));
        }
    }
}

async fn receive_update<C>(
    connection: &mut C,
    first_body: &[u8],
    content_len: usize,
) -> Result<usize, HttpOtaError>
where
    C: Read,
{
    let mut update = crate::platform::begin_ota_update()?;
    let mut received = first_body.len().min(content_len);
    crate::platform::write_ota_update(&mut update, &first_body[..received])?;

    let mut buffer = [0u8; BODY_BUFFER_LEN];
    while received < content_len {
        let remaining = content_len - received;
        let read_len = remaining.min(buffer.len());
        let count = read_io(connection, &mut buffer[..read_len]).await?;
        if count == 0 {
            return Err(HttpOtaError::BadRequest);
        }
        crate::platform::write_ota_update(&mut update, &buffer[..count])?;
        received += count;
    }

    crate::platform::finish_ota_update(update)?;
    Ok(received)
}

async fn write_response<C>(
    connection: &mut C,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), HttpOtaError>
where
    C: Write,
{
    write_all_io(connection, b"HTTP/1.1 ").await?;
    write_all_io(connection, status.as_bytes()).await?;
    write_all_io(connection, b"\r\nConnection: close\r\nContent-Type: ").await?;
    write_all_io(connection, content_type.as_bytes()).await?;
    write_all_io(connection, b"\r\nContent-Length: ").await?;
    write_usize(connection, body.len()).await?;
    write_all_io(connection, b"\r\n\r\n").await?;
    write_all_io(connection, body).await?;
    flush_io(connection).await
}

async fn write_usize<C>(connection: &mut C, mut value: usize) -> Result<(), HttpOtaError>
where
    C: Write,
{
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        write_all_io(connection, &digits[len..len + 1]).await?;
    }
    Ok(())
}

async fn read_io<C>(connection: &mut C, buffer: &mut [u8]) -> Result<usize, HttpOtaError>
where
    C: Read,
{
    match connection.read(buffer).await {
        Ok(count) => Ok(count),
        Err(_) => Err(HttpOtaError::Io),
    }
}

async fn write_all_io<C>(connection: &mut C, bytes: &[u8]) -> Result<(), HttpOtaError>
where
    C: Write,
{
    match connection.write_all(bytes).await {
        Ok(()) => Ok(()),
        Err(_) => Err(HttpOtaError::Io),
    }
}

async fn flush_io<C>(connection: &mut C) -> Result<(), HttpOtaError>
where
    C: Write,
{
    match connection.flush().await {
        Ok(()) => Ok(()),
        Err(_) => Err(HttpOtaError::Io),
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn content_length(request: &[u8]) -> Option<usize> {
    for line in request.split(|byte| *byte == b'\n') {
        let line = trim_ascii(line);
        if line.len() < "content-length:".len() {
            continue;
        }
        let (name, value) = line.split_at("content-length:".len());
        if eq_ignore_ascii_case(name, b"content-length:") {
            return parse_usize(trim_ascii(value));
        }
    }
    None
}

fn write_decimal_response(out: &mut [u8], prefix: &[u8], value: usize, suffix: &[u8]) -> usize {
    let mut len = 0;
    len += copy_into(&mut out[len..], prefix);

    let mut digits = [0u8; 20];
    let mut value = value;
    let mut digit_len = 0;
    loop {
        digits[digit_len] = b'0' + (value % 10) as u8;
        digit_len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while digit_len > 0 {
        digit_len -= 1;
        out[len] = digits[digit_len];
        len += 1;
    }

    len += copy_into(&mut out[len..], suffix);
    len
}

fn copy_into(out: &mut [u8], input: &[u8]) -> usize {
    let len = input.len().min(out.len());
    out[..len].copy_from_slice(&input[..len]);
    len
}

fn ota_error_body(error: HttpOtaError) -> &'static [u8] {
    match error {
        HttpOtaError::Io => b"Error - connection failed\n",
        HttpOtaError::BadRequest => b"Error - bad request\n",
        HttpOtaError::Ota(crate::platform::OtaError::NotAvailable) => b"Error - OTA unavailable\n",
        HttpOtaError::Ota(crate::platform::OtaError::Storage) => b"Error - flash write failed\n",
        HttpOtaError::Ota(crate::platform::OtaError::TooLarge) => b"Error - image too large\n",
        HttpOtaError::Ota(crate::platform::OtaError::InvalidImage) => b"Error - invalid image\n",
    }
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while matches!(bytes.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        bytes = &bytes[1..];
    }
    while matches!(bytes.last(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn parse_usize(bytes: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    let mut saw_digit = false;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        saw_digit = true;
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    saw_digit.then_some(value)
}
