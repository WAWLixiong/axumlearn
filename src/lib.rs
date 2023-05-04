use std::sync::Arc;
use axum::Extension;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::{Message, WebSocket};
use axum::response::IntoResponse;

use dashmap::{DashMap, DashSet};
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::warn;
use tracing_subscriber::util::SubscriberInitExt;

pub use msg::{Msg, MsgData};

mod msg;

const CAPACITY: usize = 64;

#[derive(Debug)]
struct State {
    user_rooms: DashMap<String, DashSet<String>>,
    room_users: DashMap<String, DashSet<String>>,
    tx: broadcast::Sender<Arc<Msg>>,
}

#[derive(Clone, Default)]
pub struct ChatState(Arc<State>);

impl Default for State {
    fn default() -> Self {
        let (tx, _rx) = broadcast::channel(CAPACITY);
        Self {
            user_rooms: Default::default(),
            room_users: Default::default(),
            tx,
        }
    }
}

impl ChatState {
    pub fn new() -> Self {
        Self(Arc::new(Default::default()))
    }

    pub fn get_user_rooms(&self, username: &str) -> Vec<String> {
        self.0.user_rooms
            .get(username)
            .map(|v| v.clone().into_iter().collect())
            .unwrap_or_default()
    }

    pub fn get_room_users(&self, room: &str) -> Vec<String>{
        self.0.room_users
            .get(room)
            .map(|v| v.clone().into_iter().collect())
            .unwrap_or_default()
    }
}


pub async fn ws_handler(ws: WebSocketUpgrade, Extension(state): Extension<ChatState>) -> impl IntoResponse{
    ws.on_upgrade(|socket| handle_websocket(socket, state))
}

async fn handle_websocket(socket: WebSocket, state: ChatState) {
    let mut rx = state.0.tx.subscribe();
    let (mut sender, mut receiver) = socket.split();

    let state1 = state.clone();
    let mut recv_task = tokio::spawn(async move{
        while let Some(Ok(data)) = receiver.next().await {
            if let Message::Text(msg) = data {
                // 循环了，所以需要 state1.clone(), clone比较轻量，因为只是Arc的克隆
                handle_message(msg.as_str().try_into().unwrap(), state1.clone()).await
            }
        }
    });

    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Err(e) = sender.send(Message::Text(msg.as_ref().try_into().unwrap())).await {
                warn!("error sending message: {e}");
                break;
            }
        }
    });

    tokio::select! {
        _v1 = &mut recv_task => send_task.abort(),
        _v2 = &mut send_task => recv_task.abort(),
    }

    warn!("connection closed");

    // this user has left. should send a leave message to all rooms
    // usually we can get username from auth header, here we just use 'fake_name'
    let username = "fake_name";
    let mut msg = Msg::new("fake_room".into(), username.into(), MsgData::Leave);
    for room in state.get_user_rooms(username){
        if let Err(e) = state.0.tx.send(Arc::new(Msg::leave(&room, username))) {
            warn!("error sending leave message: {e}")
        }
    }

}

async fn handle_message(msg: Msg, state: ChatState){
    let msg = match msg.data {
        MsgData::Join => {
            state
                .0
                .room_users
                .entry(msg.room.clone())
                .or_insert_with(DashSet::new)
                .insert(msg.username.clone());
            state
                .0
                .user_rooms
                .entry(msg.username.clone())
                .or_insert_with(DashSet::new)
                .insert(msg.room.clone());
            msg
        }
        MsgData::Leave => {
            if let Some(v)   = state.0.user_rooms.get_mut(&msg.username){
                v.remove(&msg.room);
                if v.is_empty(){
                    state.0.user_rooms.remove(&msg.username);
                }
            }
            if let Some(v) = state.0.room_users.get(&msg.room){
                v.remove(&msg.username);
                if v.is_empty(){
                    state.0.room_users.remove(&msg.room);
                }
            }
            msg
        }
        MsgData::Msg(_) => {
            msg
        }
    };

    if let Err(e) = state.0.tx.send(Arc::new(msg)){
        warn!("error sending message: {e}");
    }

}