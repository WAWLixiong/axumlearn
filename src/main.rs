use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use axum::{Extension, Json, Router, TypedHeader};
use axum::async_trait;
use axum::body::{boxed, Full};
use axum::extract::FromRequestParts;
use axum::headers::{Authorization, authorization::Bearer};
use axum::http::{header, StatusCode, Uri};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Server;
use jsonwebtoken as jwt;
use jsonwebtoken::Validation;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};

use axumlearn::{ChatState, ws_handler};

const SECRET: &[u8] = b"deadbeef";
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug, Deserialize, Serialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LoginResponse {
    token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Todo {
    pub id: usize,
    pub user_id: usize,
    pub title: String,
    pub completed: bool,

}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    id: usize,
    username: String,
    exp: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateTodo {
    pub title: String,
}

#[derive(Debug, Default, Clone)]
pub struct TodoStore{
    item: Arc<RwLock<Vec<Todo>>>
}


#[tokio::main]
async fn main() {
    let store = TodoStore {
        item: Arc::new(RwLock::new(
            vec![
                Todo {
                    id: 0,
                    user_id: 0,
                    title: "world".to_string(),
                    completed: false,
                }
            ]
        ))
    };

    let chat_state = Arc::new(ChatState::new());
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/todos", get(todos_handler).post(create_todo_handler).layer(Extension(store)))
        .route("/login", post(login_handler))
        .route("/ws", get(ws_handler).layer(Extension(chat_state)))
        .fallback(get(static_handler));

    let addr = SocketAddr::from(([0, 0, 0, 0], 9812));
    println!("Listening on http://{addr}");

    Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn index_handler() -> impl IntoResponse {
    static_handler("/index.html".parse().unwrap()).await
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();
    StaticFile(path)
}

async fn todos_handler(claim: Claims, Extension(store): Extension<TodoStore>) -> Result<Json<Vec<Todo>>, HttpError> {
    let user_id = claim.id;
    match store.item.read() {
        Ok(guard) => {
            Ok(Json(
                guard.iter()
                    .filter(|todo| todo.id == user_id)
                    .map(|todo| todo.clone())
                    .collect()
            ))
        }
        Err(_) => { Err(HttpError::Internal) }
    }
}

async fn create_todo_handler(claims: Claims, Extension(store): Extension<TodoStore>, Json(todo): Json<CreateTodo>) -> Result<StatusCode, HttpError> {
    println!("{:?}", claims);
    match store.item.write() {
        Ok(mut guard) => {
            let todo = Todo {
                id: get_next_id(),
                user_id: claims.id,
                title: todo.title,
                completed: false,
            };
            guard.push(todo);
            Ok(StatusCode::CREATED)
        }
        Err(_) => {
            Err(HttpError::Internal)
        }
    }
}


async fn login_handler(Json(_login): Json<LoginRequest>) -> Json<LoginResponse> {
    let claims = Claims {
        id: 1,
        username: "John".to_string(),
        exp: get_epoch() + 14 * 24 * 60 * 60,
    };
    let key = jwt::EncodingKey::from_secret(SECRET);
    let token = jwt::encode(&jwt::Header::default(), &claims, &key).unwrap();
    Json(LoginResponse { token })
}

#[async_trait]
impl<S> FromRequestParts<S> for Claims
    where
        S: Send + Sync
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(bearer)) = TypedHeader::<Authorization<Bearer>>::from_request_parts(parts, state)
            .await.map_err(|_| HttpError::Auth)?;
        let key = jwt::DecodingKey::from_secret(SECRET);
        let token = jwt::decode::<Claims>(bearer.token(), &key, &Validation::default())
            .map_err(|_| HttpError::Auth)?;
        Ok(token.claims)
    }
}

#[derive(Debug)]
enum HttpError {
    Auth,
    Internal,
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            HttpError::Auth => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            HttpError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Interval server error"),
        };
        (code, msg).into_response()
    }
}

fn get_epoch() -> usize {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
}

fn get_next_id() -> usize {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(RustEmbed)]
#[folder="myapp/build/"]
struct Assets;

struct StaticFile<T>(pub T);


impl <T> IntoResponse for StaticFile<T>
where T: Into<String>
{
    fn into_response(self) -> Response {
        let path = self.0.into();
        match Assets::get(path.as_str()) {
            Some(content) => {
                let body = boxed(Full::from(content.data));
                let mime = mime_guess::from_path(path.as_str()).first_or_octet_stream();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .body(body)
                    .unwrap()
            },
            None => {Response::builder()
                .status(StatusCode::NOT_FOUND)}
                .body(boxed(Full::from(format!("file not found {path}"))))
                .unwrap()
        }
    }
}