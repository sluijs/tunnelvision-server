//! Tunnelvision Server
//!
//! Code adapted from Axum websockets and chat examples:
//! https://github.com/tokio-rs/axum/tree/main/examples/

use std::{
    collections::HashMap,
    net::SocketAddr,
    ops::ControlFlow,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use axum_extra::headers;
use axum_extra::TypedHeader;
use clap::Parser;
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::broadcast;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// —— CLI args ——

#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    /// port to listen on
    #[arg(short = 'p', long, default_value = "8765")]
    port: u16,

    /// path to SPA/dist directory
    #[arg(short = 'd', long, default_value = "./dist")]
    static_dir: String,
}

// —— Shared state ——

#[derive(Debug, Deserialize, Serialize)]
struct GUIClientHandshake {
    connected: bool,
    hash: String,
}

struct AppState {
    clients: Arc<Mutex<HashMap<String, SocketAddr>>>,
    tx: broadcast::Sender<Message>,
}

impl Default for AppState {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            tx,
        }
    }
}

// —— Main ——

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "tunnelvision_server=debug,tower_http=debug,axum::rejection=trace".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app_state = Arc::new(AppState::default());
    let static_dir = args.static_dir.clone();

    let app = Router::new()
        // WebSocket endpoints
        .route("/api/hello", get(hello))
        .route("/ws", get(ws_handler))
        // Fallback service for static files and SPA
        .fallback_service(
            ServeDir::new(args.static_dir).not_found_service(get(move || async move {
                let index_path = PathBuf::from(static_dir).join("index.html");
                match fs::read_to_string(index_path).await {
                    Ok(content) => (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        content,
                    )
                        .into_response(),
                    Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
                }
            })),
        )
        // CORS & Tracing
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        )
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    println!("--- tunnelvision ---");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

// —— Handlers ——

async fn hello() -> impl IntoResponse {
    "Hello, Client!"
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let ua = user_agent
        .map(|TypedHeader(h)| h.to_string())
        .unwrap_or_else(|| "Unknown".into());
    println!("`{ua}` at {addr} connected.");

    ws.max_frame_size(256 * 1024 * 1024)
        .max_message_size(256 * 1024 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, addr, state))
}

async fn handle_socket(socket: WebSocket, who: SocketAddr, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // initial ping
    if sender
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .is_err()
    {
        eprintln!("failed to ping {}", who);
        return;
    }

    let mut rx = state.tx.subscribe();

    let send_task = {
        let state = state.clone();
        tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                match &msg {
                    Message::Text(_) => {
                        if sender.send(msg.clone()).await.is_err() {
                            break;
                        }
                    }
                    Message::Binary(d) if d.len() > 22 => {
                        let (hash, data) = d.split_at(22);
                        if let Ok(hash) = String::from_utf8(hash.to_vec()) {
                            // Corrected code
                            let should_send = {
                                // Lock is acquired and released within this new scope
                                let clients = state.clients.lock().unwrap();
                                clients.get(&hash) == Some(&who)
                            };

                            if should_send {
                                if sender
                                    .send(Message::Binary(data.to_vec().into()))
                                    .await // <-- Lock is gone, it's safe to await now!
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        })
    };

    let recv_task = {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    Message::Text(t) => {
                        if let Ok(handshake) = serde_json::from_str::<GUIClientHandshake>(&t) {
                            state.clients.lock().unwrap().insert(handshake.hash, who);
                        }
                        let _ = state.tx.send(Message::Text(t));
                    }
                    Message::Binary(d) => {
                        let _ = state.tx.send(Message::Binary(d));
                    }
                    Message::Close(cf) => {
                        state.clients.lock().unwrap().retain(|_, &mut v| v != who);
                        if let Some(cf) = cf {
                            println!("{} closed: {} {:?}", who, cf.code, cf.reason);
                        }
                        return ControlFlow::Break(());
                    }
                    _ => {}
                }
            }
            ControlFlow::Continue(())
        })
    };

    // Corrected code
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    };
}