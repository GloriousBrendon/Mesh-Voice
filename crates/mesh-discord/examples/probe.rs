//! Connects to the running engine and prints the first snapshot.
//! `cargo run -p mesh-discord --example probe`
#[tokio::main]
async fn main() {
    let socket = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_default()).join("discord-engine.sock");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(mesh_discord::run(socket, tx));
    while let Some(event) = rx.recv().await {
        match event {
            mesh_discord::Event::Connected(_) => println!("connected"),
            mesh_discord::Event::Snapshot(s) => {
                println!("me={:?} voice={:?} guilds={}", s.me.map(|m| m.name), s.voice.channel_id, s.guilds.len());
                return;
            }
            mesh_discord::Event::Disconnected => println!("disconnected"),
        }
    }
}
