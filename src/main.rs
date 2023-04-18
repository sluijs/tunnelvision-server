//! Tunnelvision Server
//!
//! Code adapted from Axum websockets and chat examples:
//! https://github.com/tokio-rs/axum/tree/main/examples/
//!
//! Run the server with
//! ```not_rust
//! cargo run
//! ```
//!
//! Run a browser client with
//! ```not_rust
//! firefox http://localhost:8765/ws
//! ```

use std::sync::Mutex;
use std::collections::HashMap;
use std::{net::SocketAddr, ops::ControlFlow, path::PathBuf, sync::Arc};

use axum::{Router, routing::get};
use axum::body::{boxed, Body};
use axum::extract::{State, TypedHeader};
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use clap::Parser;
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::broadcast;
use tower::{ServiceExt};
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Parse CLI arguments using Clap
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short = 'p', long = "port", default_value = "8765")]
    port: u16,

    #[arg(short = 'd', long = "static_dir", default_value = "./dist")]
    static_dir: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GUIClientHandshake {
    connected: bool,
    hash: String,
}

struct AppState {
    // Store the address of all connected clients, with a hash that uniquely identifies the client
    clients: Arc<Mutex<HashMap<String, SocketAddr>>>,

    // Broadcast channel for sending messages to all clients
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

#[tokio::main]
async fn main() {
    // Parse CLI arguments
    let args = Args::parse();

    // Set the logging information
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "example_websockets=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Create the app state
    let app_state = Arc::new(AppState::default());

    // build our application with some routes
    let app = Router::new()
        // Setup a WebSocket route
        .route("/api/hello", get(hello))
        .route("/ws", get(ws_handler))
        .with_state(app_state)

        // Serve static files, such as the SPA
        .fallback_service(get(|req| async move {
            match ServeDir::new(&args.static_dir).oneshot(req).await {
                Ok(res) => {
                    let status = res.status();
                    match status {
                        StatusCode::NOT_FOUND => {
                            let index_path = PathBuf::from(&args.static_dir).join("index.html");
                            let index_content = match fs::read_to_string(index_path).await {
                                Err(_) => {
                                    return Response::builder()
                                        .status(StatusCode::NOT_FOUND)
                                        .body(boxed(Body::from("index file not found")))
                                        .unwrap()
                                }
                                Ok(index_content) => index_content,
                            };

                            Response::builder()
                                .status(StatusCode::OK)
                                .body(boxed(Body::from(index_content)))
                                .unwrap()
                        }
                        _ => res.map(boxed),
                    }
                }
                Err(err) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(boxed(Body::from(format!("error: {err}"))))
                    .expect("error response"),
            }
        }))

        // Logging so we can see whats going on
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        );

    // run it with hyper
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    tracing::debug!("--- listening on {}", addr);

    let server = axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>());

    println!("--- tunnelvision ---");

    if let Err(err) = server.await {
        tracing::error!("--- server error: {err}");
    }

}

/// The handler for the HTTP request (this gets called when the HTTP GET lands at the start
/// of websocket negotiation). After this completes, the actual switching from HTTP to
/// websocket protocol will occur.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client
/// as well as things from HTTP headers such as user-agent of the browser etc.
async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };

    println!("--- `{user_agent}` at {addr} connected.");

    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    let max_size = 256 * 1024 * 1024;
    ws.max_frame_size(max_size)
        .max_message_size(max_size)
        .on_upgrade(move |socket| handle_socket(socket, addr, state))
}

/// Actual websocket statemachine (one will be spawned per connection)
async fn handle_socket(socket: WebSocket, who: SocketAddr, state: Arc<AppState>) {
    // By splitting socket we can send and receive at the same time. In this example we will send
    // unsolicited messages to client based on some sort of server's internal event (i.e .timer).
    let (mut sender, mut receiver) = socket.split();

    // Send a ping (unsupported by some browsers) just to kick things off and get a response
    if sender.send(Message::Ping(vec![1, 2, 3])).await.is_ok() {
        println!("--- pinged {}...", who);
    } else {
        println!("Could not send ping {}!", who);
        // no Error here since the only thing we can do is to close the connection.
        // If we can not send messages, there is no way to salvage the statemachine anyway.
        return;
    }

    // Subscribe to the broadcast channel
    let mut rx = state.clone().tx.subscribe();

    // Spawn the first task that will receive broadcast messages and send text
    // messages over the websocket to our client.
    let s = state.clone();
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            // In any websocket error, break loop.
            match msg {
                // Forward text messages to all clients, including the origin
                Message::Text(_) => {
                    if sender.send(msg).await.is_err() {
                        break;
                    }
                },

                // Forward binary messages to a receiver with a matching hash
                Message::Binary(d) => {
                    if d.len() > 22 {
                        let (hash, data) = d.split_at(22);
                        if let Ok(hash) = String::from_utf8(hash.to_vec()) {
                            let clients = s.clients.lock().unwrap().clone();
                            if let Some(client) = clients.get(&hash) {
                                if client == &who {
                                    println!("--- {} accepted binary message", who);
                                    if sender.send(Message::Binary(Vec::from(data))).await.is_err() {
                                        break;
                                    }
                                }
                            } else {
                                println!("--- {} rejected binary message", who);
                            }
                        }
                    }
                },
                _ => {}
            }
        }
    });


    let s = state.clone();
    // Spawn a task that receives messages from the websocket and forwards them to the broadcast channel
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                // Messages are forwarded to all clients, including the sender; also handles new connections
                Message::Text(t) => {
                    println!(">>> {} sent str: {:?}", who, t);
                    if let Ok(client) = serde_json::from_str::<GUIClientHandshake>(&t) {
                        // Add client to the list of clients
                        println!("--- {} added to client list", who);
                        s.clone().clients.lock().unwrap().insert(client.hash, who);
                    }

                    // // Forward the message to _all_ clients
                    s.tx.send(Message::Text(t)).expect("Could not send message to broadcast channel");
                }

                Message::Binary(d) => {
                    // Forward the message to all clients
                    println!(">>> {} sent {} bytes", who, d.len());
                    s.tx.send(Message::Binary(d)).expect("Could not send message to broadcast channel");
                }

                Message::Close(c) => {
                    if let Some(cf) = c {
                        println!(
                            ">>> {} sent close with code {} and reason `{}`",
                            who, cf.code, cf.reason
                        );
                    } else {
                        println!(">>> {} somehow sent close message without CloseFrame", who);
                    }

                    // Remove client from the list of clients
                    println!("--- {} removed from client list", &who);
                    s.clone().clients.lock().unwrap().retain(|_, v| v != &who);

                    println!("--- updated client list: ");
                    for (k, v) in s.clone().clients.lock().unwrap().iter() {
                        println!("--- {} -> {}", k, v);
                    }

                    return ControlFlow::Break(());
                }
                Message::Pong(v) => {
                    println!(">>> {} sent pong with {:?}", who, v);
                }
                // You should never need to manually handle Message::Ping, as axum's websocket library
                // will do so for you automagically by replying with Pong and copying the v according to
                // spec. But if you need the contents of the pings you can see them here.
                Message::Ping(v) => {
                    println!(">>> {} sent ping with {:?}", who, v);
                }
            }
        }

        ControlFlow::Continue(())
    });


    // If any one of the tasks run to completion, we abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

}

// Simple handler to ping the server
async fn hello() -> impl IntoResponse {
    "Hello, Client!"
}
