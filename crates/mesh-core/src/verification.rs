//! Interactive device verification (emoji SAS), bridged to the frontend
//! through [`MeshUpdate`] events on the client's update channel.

use futures_util::StreamExt;
use matrix_sdk::{
    Client,
    encryption::verification::{
        SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
    },
    ruma::events::{
        key::verification::request::ToDeviceKeyVerificationRequestEvent,
        room::message::{MessageType, OriginalSyncRoomMessageEvent},
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::MeshUpdate;

/// Registers event handlers that surface incoming verification requests
/// (both to-device and in-room flavors) on the update channel.
pub(crate) fn register_handlers(client: &Client, tx: UnboundedSender<MeshUpdate>) {
    client.add_event_handler({
        let tx = tx.clone();
        move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
            let tx = tx.clone();
            async move {
                let request = client
                    .encryption()
                    .get_verification_request(&ev.sender, &ev.content.transaction_id)
                    .await;
                if let Some(request) = request {
                    announce(request, tx);
                }
            }
        }
    });

    client.add_event_handler({
        let tx = tx.clone();
        move |ev: OriginalSyncRoomMessageEvent, client: Client| {
            let tx = tx.clone();
            async move {
                if let MessageType::VerificationRequest(_) = &ev.content.msgtype {
                    let request = client
                        .encryption()
                        .get_verification_request(&ev.sender, &ev.event_id)
                        .await;
                    if let Some(request) = request {
                        announce(request, tx);
                    }
                }
            }
        }
    });
}

fn announce(request: VerificationRequest, tx: UnboundedSender<MeshUpdate>) {
    let _ = tx.send(MeshUpdate::VerificationRequested {
        user_id: request.other_user_id().to_owned(),
        flow_id: request.flow_id().to_owned(),
    });
    tokio::spawn(watch_request(request, tx));
}

/// Follows a verification request until it transitions into SAS, finishes,
/// or gets cancelled.
async fn watch_request(request: VerificationRequest, tx: UnboundedSender<MeshUpdate>) {
    let flow_id = request.flow_id().to_owned();
    let mut changes = request.changes();
    while let Some(state) = changes.next().await {
        match state {
            VerificationRequestState::Transitioned { verification } => {
                if let Verification::SasV1(sas) = verification {
                    watch_sas(sas, flow_id, tx).await;
                }
                return;
            }
            VerificationRequestState::Done => {
                let _ = tx.send(MeshUpdate::VerificationDone { flow_id });
                return;
            }
            VerificationRequestState::Cancelled(info) => {
                let _ = tx.send(MeshUpdate::VerificationCancelled {
                    flow_id,
                    reason: info.reason().to_owned(),
                });
                return;
            }
            _ => {}
        }
    }
}

/// Accepts the SAS start (when the other side initiated it) and streams
/// emoji/done/cancelled states to the frontend.
async fn watch_sas(sas: SasVerification, flow_id: String, tx: UnboundedSender<MeshUpdate>) {
    if !sas.we_started()
        && let Err(err) = sas.accept().await
    {
        tracing::warn!("failed to accept SAS verification: {err}");
        let _ = tx.send(MeshUpdate::VerificationCancelled {
            flow_id,
            reason: err.to_string(),
        });
        return;
    }

    let mut changes = sas.changes();
    while let Some(state) = changes.next().await {
        match state {
            SasState::KeysExchanged { emojis, .. } => {
                if let Some(emojis) = emojis {
                    let emojis = emojis
                        .emojis
                        .iter()
                        .map(|e| crate::SasEmoji {
                            symbol: e.symbol.to_owned(),
                            description: e.description.to_owned(),
                        })
                        .collect();
                    let _ = tx.send(MeshUpdate::VerificationEmojis {
                        flow_id: flow_id.clone(),
                        emojis,
                    });
                }
            }
            SasState::Done { .. } => {
                let _ = tx.send(MeshUpdate::VerificationDone { flow_id });
                return;
            }
            SasState::Cancelled(info) => {
                let _ = tx.send(MeshUpdate::VerificationCancelled {
                    flow_id,
                    reason: info.reason().to_owned(),
                });
                return;
            }
            _ => {}
        }
    }
}
