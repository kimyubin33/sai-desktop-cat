//! The short-lived browser page for returning to Sai after a Google consent flow.
use super::{oauth, read_headers, LocalRequest, ReturnToApp};
use std::time::Duration;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

const TEMPLATE: &str = include_str!("return_page.html");
const MARK: &str = include_str!("../../../../ui/assets/sai-mark.svg");
const PAGE_LIFETIME: Duration = Duration::from_secs(300);

pub(super) enum Response<'a> {
    BadRequest,
    Redirect(&'a str),
    Page(String),
}

pub(super) async fn write_response(socket: &mut TcpStream, response: Response<'_>) {
    let (status, extra_headers, body) = match response {
        Response::BadRequest => (
            "400 Bad Request",
            "Referrer-Policy: no-referrer\r\n".into(),
            "<!doctype html><html lang=\"ko\"><meta charset=\"utf-8\"><title>사이</title><p>사이 앱에서 로그인 창을 다시 열어 주세요.</p></html>".into(),
        ),
        Response::Redirect(path) => (
            "303 See Other",
            format!("Location: {path}\r\nReferrer-Policy: no-referrer\r\n"),
            String::new(),
        ),
        Response::Page(html) => ("200 OK", "Referrer-Policy: same-origin\r\n".into(), html),
    };
    let message = format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ =
        tokio::time::timeout(Duration::from_secs(2), socket.write_all(message.as_bytes())).await;
}

fn render(purpose: oauth::Purpose, ok: bool, returned: bool, path: &str) -> String {
    let title = if ok {
        match purpose {
            oauth::Purpose::Identity | oauth::Purpose::Files => "Google 연결 완료",
            oauth::Purpose::Calendar => "Google Calendar 연결 완료",
            oauth::Purpose::Folder | oauth::Purpose::SelectedFiles => "자료 선택 완료",
        }
    } else {
        "연결을 완료하지 못했어요"
    };
    let (copy, button) = if returned {
        ("사이 창에서 이어서 진행해 주세요.", "사이 다시 열기")
    } else if ok {
        ("사이 앱에서 이어서 사용해 주세요.", "사이로 돌아가기")
    } else {
        ("사이 앱에서 연결 상태를 확인해 주세요.", "사이로 돌아가기")
    };
    TEMPLATE
        .replace("{{MARK}}", MARK)
        .replace("{{TITLE}}", title)
        .replace("{{COPY}}", copy)
        .replace("{{BUTTON}}", button)
        .replace("{{PATH}}", path)
}

pub(super) async fn serve(
    listener: TcpListener,
    mut callback_socket: TcpStream,
    host: String,
    purpose: oauth::Purpose,
    ok: bool,
    return_to_app: Option<ReturnToApp>,
) {
    // A fresh path identifies only this completion page. Neither the OAuth
    // code nor the state appears in the redirect, markup, or return request.
    let path = format!("/complete/{}", oauth::random_token());
    write_response(&mut callback_socket, Response::Redirect(&path)).await;
    drop(callback_socket);

    let origin = format!("http://{host}");
    let returned = format!("{path}/returned");
    let responses = async {
        let mut auto_return = ok;
        while let Ok((mut socket, _)) = listener.accept().await {
            let headers = read_headers(&mut socket).await;
            let request = headers
                .as_deref()
                .and_then(|h| LocalRequest::parse(h, &host));
            let (response, reveal) = match request {
                Some(r) if r.method == "GET" && r.path == path => (
                    Response::Page(render(purpose, ok, false, &path)),
                    std::mem::take(&mut auto_return),
                ),
                Some(r) if r.method == "GET" && r.path == returned => {
                    (Response::Page(render(purpose, ok, true, &path)), false)
                }
                Some(r)
                    if r.method == "POST"
                        && r.path == path
                        && r.header("origin") == Some(origin.as_str()) =>
                {
                    auto_return = false;
                    (Response::Redirect(&returned), true)
                }
                _ => (Response::BadRequest, false),
            };
            write_response(&mut socket, response).await;
            drop(socket);
            if reveal {
                if let Some(reveal) = &return_to_app {
                    reveal();
                }
            }
        }
    };
    let _ = tokio::time::timeout(PAGE_LIFETIME, responses).await;
}
