use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
    thread::JoinHandle,
    time::Duration,
};

use crossbeam_channel::Receiver;
use jito_protos::shredstream::{
    shredstream_proxy_server::{ShredstreamProxy, ShredstreamProxyServer},
    Entry as PbEntry, SubscribeEntriesRequest,
};
use log::{debug, info};
use tokio::sync::broadcast::{Receiver as BroadcastReceiver, Sender};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

// 新增 import
use crate::parser::{parse_entry, ParsedEntry};

#[derive(Debug)]
pub struct ShredstreamProxyService {
    entry_sender: Arc<Sender<PbEntry>>,
}

pub fn start_server_thread(
    addr: SocketAddr,
    entry_sender: Arc<Sender<PbEntry>>,
    exit: Arc<AtomicBool>,
    shutdown_receiver: Receiver<()>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let server_handle = runtime.spawn(async move {
            info!("starting server on {:?}", addr);
            tonic::transport::Server::builder()
                .add_service(ShredstreamProxyServer::new(ShredstreamProxyService {
                    entry_sender,
                }))
                .serve(addr)
                .await
                .unwrap();
        });

        while !exit.load(std::sync::atomic::Ordering::Relaxed) {
            if shutdown_receiver
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
            {
                server_handle.abort();
                info!("shutting down entries server");
                break;
            }
        }
    })
}

#[tonic::async_trait]
impl ShredstreamProxy for ShredstreamProxyService {
    type SubscribeEntriesStream = ReceiverStream<Result<ParsedEntry, tonic::Status>>;

    async fn subscribe_entries(
        &self,
        _request: tonic::Request<SubscribeEntriesRequest>,
    ) -> Result<tonic::Response<Self::SubscribeEntriesStream>, tonic::Status> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let mut entry_receiver: BroadcastReceiver<PbEntry> = self.entry_sender.subscribe();

        tokio::spawn(async move {
            while let Ok(entry) = entry_receiver.recv().await {
                // 調用解析邏輯
                let parsed_entry = parse_entry(entry);

                match tx.send(Ok(parsed_entry)).await {
                    Ok(_) => (),
                    Err(_e) => {
                        debug!("client disconnected");
                        break;
                    }
                }
            }
        });

        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }
}
