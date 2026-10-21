use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
fn main() {
    let close = std::env::args().any(|x| x == "hello");
    tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap().block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", if close {18478} else {18476})).await.unwrap();
        let mut served = 0;
        while served < 1024 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = Vec::with_capacity(4096);
            loop {
                let mut headers = [httparse::EMPTY_HEADER; 100];
                let mut req = httparse::Request::new(&mut headers);
                if let httparse::Status::Complete(end) = req.parse(&buf).unwrap() {
                    assert_eq!(req.method, Some("GET"));
                    assert_eq!(req.path, Some("/"));
                    stream.write_all(if close {
                        b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello, world"
                    } else {
                        b"HTTP/1.1 200 \r\nContent-Type: text/plain\r\nContent-Length: 12\r\nConnection: keep-alive\r\n\r\nHello, world"
                    }).await.unwrap();
                    served += 1;
                    buf.drain(..end);
                    if close || served == 1024 {break;}
                } else {
                    assert!(stream.read_buf(&mut buf).await.unwrap() > 0);
                }
            }
        }
        println!("1024");
    });
}
