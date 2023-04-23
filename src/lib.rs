use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::{Message, WebSocket};
use axum::response::IntoResponse;
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::sync::broadcast;

pub use msg::{Msg, MsgData};

mod msg;


#[derive(Debug)]
pub struct WebSocketStore {
    pub user_set: Mutex<HashSet<String>>,
    pub tx: broadcast::Sender<String>,
}

#[derive(Clone)]
pub struct AppStore(Arc<WebSocketStore>);


pub async fn ws_handler(ws: WebSocketUpgrade, Extension(websocket_store): Extension<AppStore>) -> impl IntoResponse {
    ws.on_upgrade(|socket| { websocket(socket, websocket_store) })
}

async fn websocket(socket: WebSocket, websocket_store: AppStore) {
    // 服务器和客户端(浏览器)通讯
    let (mut sender, mut receiver) = socket.split();

    let mut username = String::new();

    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(name) = msg {
            check_user_name(&mut username, name.as_ref(), &websocket_store.0);
            if username.is_empty() {
                // 用户名已存在
                break;
            } else {
                let _ = sender.send(Message::Text(String::from("username already taken"))).await;
                return;
            }
        }
    }

    let mut rx = websocket_store.0.tx.subscribe();

    // 广播到大厅 username joined
    let _ = websocket_store.0.tx.send(format!("{username} joined"));

    let mut send_task = tokio::spawn(async move {
        // 接收大厅的消息，发送给浏览器客户端
        while let Ok(msg) = rx.recv().await {
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let tx = websocket_store.0.tx.clone();
    let name = username.clone();

    let mut recv_task = tokio::spawn(async move {
        // 接收浏览器客户端的消息，发送到大厅
        while let Some(Ok(Message::Text(msg))) = receiver.next().await {
            let _ = tx.send(format!("{name} said: {msg}"));
        }
    });

    // If any one of the tasks run to completion, we abort the other
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }

    // 广播到大厅 username left
    let _ = websocket_store.0.tx.send(format!("{username} left"));
    websocket_store.0.user_set.lock().unwrap().remove(&username);
}

fn check_user_name(username_string: &mut String, name: &str, websocket_store: &Arc<WebSocketStore>) {
    let user_set = websocket_store.user_set.lock().unwrap();
    if !user_set.contains(name) {
        username_string.push_str(name)
    }
}