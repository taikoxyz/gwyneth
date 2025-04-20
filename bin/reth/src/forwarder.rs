use axum::{
    extract::State,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use alloy_json_rpc::{Request as AlloyRequest, Response as AlloyResponse};
use anyhow::Result;
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

static DEFAULT_CHAIN_ID: u64 = 160010;

/// Shared application state holding:
/// - `Client` from Reqwest to make outgoing calls
/// - A string for the remote JSON-RPC endpoint URL
/// - An `active_chain_id` that is updated via `eth_setActiveChainId`
#[derive(Clone)]
struct AppState {
    client: Client,
    chains: HashMap<u64, String>,
    active_chain_id: u64,
}

pub async fn start_forwarder() -> Result<()> {
    let mut chains = HashMap::new();
    chains.insert(160010, "http://127.0.0.1:32002".to_string());
    chains.insert(167010, "http://127.0.0.1:32005".to_string());
    chains.insert(167011, "http://127.0.0.1:32006".to_string());

    // Create shared application state
    let state = AppState {
        client: Client::new(),
        chains,
        active_chain_id: DEFAULT_CHAIN_ID,
    };

    // Wrap the state in an Arc<Mutex<>> so multiple requests can modify it safely
    let shared_state = Arc::new(Mutex::new(state));

    // Create Axum router with one POST route
    let app = Router::new()
        .route("/", post(handle_jsonrpc))
        .with_state(shared_state);

    // Start the server
    //let addr = SocketAddr::from(([127, 0, 0, 1], 32009));
    //println!("JSON-RPC forwarder listening on {}", addr);

    // axum::Server::bind(&addr)
    //     .serve(app.into_make_service())
    //     .await
    //     .unwrap();

    println!("listening...");

    // build our application with a single route
    // let app = Router::new().route("/", get(|| async { "Hello, World!" }));

    // // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("127.0.0.1:32009").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Handler for incoming JSON-RPC requests. Uses Alloy to parse the request
/// and then either:
///  1) Handle locally if the method is `eth_setActiveChainId` or `eth_getActiveChainId`.
///  2) Forward the raw payload to a remote JSON-RPC endpoint for all other methods.
async fn handle_jsonrpc(
    State(state_mutex): State<Arc<Mutex<AppState>>>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    // First, parse the incoming JSON as an Alloy JSON-RPC request.
    // AlloyRequest is the JSON-RPC request type from Alloy, which validates the structure.
    // let parse_result: Result<AlloyRequest<SingleParam>, _> = serde_json::from_value(payload.clone());
    // let request = match parse_result {
    //     Ok(req) => req,
    //     Err(e) => {
    //         // If it fails Alloy's JSON-RPC parsing, return an error message
    //         let error_response = json!({
    //             "jsonrpc": "2.0",
    //             "error": {
    //                 "code": -32600,
    //                 "message": format!("Invalid JSON-RPC request: {e}")
    //             },
    //             "id": null
    //         });
    //         return Json(error_response);
    //     }
    // };

    println!("payload: {:?}", payload);
    println!("method: {:?}", payload["method"]);

    // // Retrieve the method name to determine if we handle locally or forward
    // let method = request.meta.method.into_owned().as_str();

    // // For JSON-RPC, the request ID could be None, a Number, or a String.
    // // We'll forward or return the same ID to keep it consistent for the client.
    // let request_id = request.meta.id.clone();

    let method = payload["method"].as_str().unwrap();
    let request_id = payload["id"].as_u64().unwrap();

    // // Lock the shared state for reading/updating
    let mut state: tokio::sync::MutexGuard<'_, AppState> = state_mutex.lock().await;

    match method {
        "eth_setActiveChainId" => {
            let chain_id = payload["params"][0].clone();
            let chain_id = chain_id.as_u64().unwrap();
            println!("set active chain to: {:?}", chain_id);

            let chain_id = if chain_id == 0 {
                DEFAULT_CHAIN_ID
            } else {
                chain_id
            };

            if state.chains.contains_key(&chain_id) {
                // Update the active_chain_id
                state.active_chain_id = chain_id;

                // Return a successful JSON-RPC response (could be null or the new chain id)
                let success_response = json!({
                    "jsonrpc": "2.0",
                    "result": true,
                    "id": request_id
                });
                Json(success_response)
            } else {
                // Return a successful JSON-RPC response (could be null or the new chain id)
                let success_response = json!({
                    "jsonrpc": "2.0",
                    "result": false,
                    "id": request_id
                });
                Json(success_response)
            }
        }

        "eth_getActiveChainId" => {
            // Return the locally stored chain ID
            let chain_id = state.active_chain_id;
            let success_response = json!({
                "jsonrpc": "2.0",
                "result": state.active_chain_id,
                "id": request_id
            });
            Json(success_response)
        }

        // Forward all other methods
        _ => {
            println!("forwarding to chain {}", state.active_chain_id);
            let remote_url = &state.chains[&state.active_chain_id];
            let client = &state.client;

            // Forward the exact JSON payload to the remote endpoint
            // and wait for a JSON response
            let forward_result = client
                .post(remote_url)
                .json(&payload)
                .send()
                .await;

            println!("forward result: {:?}", forward_result);

            let Ok(response) = forward_result else {
                // If the forward request fails, return an error JSON-RPC response
                let error_json = json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32603,
                        "message": "Error forwarding request to remote server"
                    },
                    "id": request_id
                });
                return Json(error_json);
            };

            // Attempt to parse the remote server's JSON response as a valid Alloy JSON-RPC Response
            let remote_json = match response.json::<Value>().await {
                Ok(val) => val,
                Err(_) => {
                    let error_json = json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32603,
                            "message": "Invalid response from remote server"
                        },
                        "id": request_id
                    });
                    return Json(error_json);
                }
            };

            // Check if the remote response is a valid JSON-RPC response
            let parse_response: Result<AlloyResponse, _> = serde_json::from_value(remote_json.clone());
            if parse_response.is_err() {
                let error_json = json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32603,
                        "message": "Remote server sent an invalid JSON-RPC response"
                    },
                    "id": request_id
                });
                return Json(error_json);
            }

            // Return the remote server's response directly to the client
            Json(remote_json)
        }
    }
}
