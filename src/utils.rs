use uuid::Uuid;

pub fn get_rand_guid() -> String {
    let guid = Uuid::new_v4();
    return guid.to_hyphenated().to_string();
}

pub fn rm_newline(line:String) -> String {
    let mut l = line;
    if (l.ends_with("\n")) {
        l = l[0..l.len() - 1].to_string();
    }
    return l;
}

//implementing own read_line because this app cannot use BufReader due to how it passes the stream to a thread to be proxied. BufReader will advance the stream when it buffers, which results in data loss
//returns when '\n' is reached, and it includes '\n'. 
//requires line_buf is maintained externally (reset when a line is read)
pub async fn tokio_read_line(stream:&tokio::net::TcpStream, line_bytes:&mut Vec<u8>) -> tokio::io::Result<()> {
    loop {
        stream.readable().await?;

        let mut buf = vec![0; 1];

        match stream.try_read(&mut buf) {
            Err(e) if e.kind() == tokio::io::ErrorKind::WouldBlock => {

            },
            Err(e) if e.kind() == tokio::io::ErrorKind::UnexpectedEof => {
                break;
            },
            Err(e) => {
                return Err(e);
            },
            Ok(bytes_read) => {
                if (bytes_read > 0) {
                    line_bytes.push(buf[0]);
                    if (buf[0] == b"\n"[0]) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }
    return Ok(());
}

pub async fn tokio_write_line(stream:&mut tokio::net::TcpStream, line:&str) -> tokio::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    stream.write_all((line.to_owned() + "\n").as_bytes()).await?;
    return Ok(());
}