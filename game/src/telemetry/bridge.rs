use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};
use rb_shared::events::CombatEvent;
use rb_shared::proto::telemetry::telemetry_service_client::TelemetryServiceClient;
use rb_shared::proto::telemetry::SendEventsRequest;

#[derive(Resource)]
pub struct TelemetryChannel {
    pub sender: Sender<CombatEvent>,
}

pub fn spawn_telemetry_thread(receiver: Receiver<CombatEvent>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        rt.block_on(async move {
            // Lazy connection - won't block if server isn't up yet
            let channel = tonic::transport::Channel::from_static("http://localhost:50051")
                .connect_lazy();
            let mut client = TelemetryServiceClient::new(channel);

            let mut buffer: Vec<CombatEvent> = Vec::new();
            let mut last_flush = std::time::Instant::now();

            loop {
                match receiver.try_recv() {
                    Ok(event) => buffer.push(event),
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                }

                let should_flush = buffer.len() >= 16
                    || (last_flush.elapsed().as_secs() >= 2 && !buffer.is_empty());

                if should_flush {
                    let proto_events: Vec<_> =
                        buffer.drain(..).map(Into::into).collect();
                    let count = proto_events.len();

                    let request = tonic::Request::new(SendEventsRequest {
                        events: proto_events,
                    });

                    match client.send_events(request).await {
                        Ok(response) => {
                            eprintln!(
                                "[telemetry] gRPC: sent {} events (ack: {})",
                                count,
                                response.into_inner().events_received
                            );
                        }
                        Err(e) => {
                            eprintln!("[telemetry] gRPC send failed: {e}");
                        }
                    }
                    last_flush = std::time::Instant::now();
                }

                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        });
    });
}
