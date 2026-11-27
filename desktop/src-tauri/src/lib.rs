pub mod bridge;

use std::sync::Arc;

use bridge::{BridgeManager, ExampleSummary, HandshakeState};
use serde_json::Value;
use tauri::{Manager, RunEvent, State};

struct ManagedBridge(Arc<BridgeManager>);

#[tauri::command]
async fn bridge_handshake(state: State<'_, ManagedBridge>) -> Result<HandshakeState, String> {
    let manager = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || manager.handshake())
        .await
        .map_err(|err| format!("后端任务调度失败：{err}"))?
}

#[tauri::command]
async fn bridge_restart(state: State<'_, ManagedBridge>) -> Result<HandshakeState, String> {
    let manager = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || manager.restart())
        .await
        .map_err(|err| format!("重启后端任务失败：{err}"))?
}

#[tauri::command]
async fn bridge_call(state: State<'_, ManagedBridge>, command: Value) -> Result<Value, String> {
    let manager = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || manager.call(command))
        .await
        .map_err(|err| format!("后端调用任务失败：{err}"))?
}

#[tauri::command]
async fn list_examples() -> Result<Vec<ExampleSummary>, String> {
    tauri::async_runtime::spawn_blocking(bridge::list_examples)
        .await
        .map_err(|err| format!("枚举内置示例失败：{err}"))?
}

#[tauri::command]
async fn read_example(file_name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || bridge::read_example_text(&file_name))
        .await
        .map_err(|err| format!("读取内置示例失败：{err}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ManagedBridge(Arc::new(BridgeManager::new())))
        .invoke_handler(tauri::generate_handler![
            bridge_handshake,
            bridge_restart,
            bridge_call,
            list_examples,
            read_example,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Reliner desktop application");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            let state = app_handle.state::<ManagedBridge>();
            state.0.shutdown();
        }
    });
}
