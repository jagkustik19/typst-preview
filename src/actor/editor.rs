use std::sync::{Arc, RwLock};

use futures::{SinkExt, StreamExt};
use log::{debug, info, trace, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::{net::TcpStream, sync::broadcast};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use typst_ts_core::debug_loc::DocumentPosition;

use crate::debug_loc::{InternQuery, SpanInterner};
use crate::outline::Outline;
use crate::{
    actor::typst::TypstActorRequest, ChangeCursorPositionRequest, DocToSrcJumpInfo, MemoryFiles,
    MemoryFilesShort, SrcToDocJumpRequest,
};

use super::webview::WebviewActorRequest;
#[derive(Debug, Deserialize)]
pub struct DocToSrcJumpResolveRequest {
    /// Span id in hex-format.
    pub span: String,
}

#[derive(Debug, Deserialize)]
pub struct PanelScrollByPositionRequest {
    position: DocumentPosition,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum CompileStatus {
    Compiling,
    CompileSuccess,
    CompileError,
}

#[derive(Debug)]
pub enum EditorActorRequest {
    DocToSrcJumpResolve(DocToSrcJumpResolveRequest),
    DocToSrcJump(DocToSrcJumpInfo),
    Outline(Outline),
    CompileStatus(CompileStatus),
}

pub struct EditorActor {
    mailbox: mpsc::UnboundedReceiver<EditorActorRequest>,
    editor_websocket_conn: WebSocketStream<TcpStream>,

    world_sender: mpsc::UnboundedSender<TypstActorRequest>,
    webview_sender: broadcast::Sender<WebviewActorRequest>,

    span_interner: Arc<RwLock<SpanInterner>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event")]
enum ControlPlaneMessage {
    #[serde(rename = "changeCursorPosition")]
    ChangeCursorPosition(ChangeCursorPositionRequest),
    #[serde(rename = "panelScrollTo")]
    SrcToDocJump(SrcToDocJumpRequest),
    #[serde(rename = "panelScrollByPosition")]
    PanelScrollByPosition(PanelScrollByPositionRequest),
    #[serde(rename = "sourceScrollBySpan")]
    DocToSrcJumpResolve(DocToSrcJumpResolveRequest),
    #[serde(rename = "syncMemoryFiles")]
    SyncMemoryFiles(MemoryFiles),
    #[serde(rename = "updateMemoryFiles")]
    UpdateMemoryFiles(MemoryFiles),
    #[serde(rename = "removeMemoryFiles")]
    RemoveMemoryFiles(MemoryFilesShort),
}

#[derive(Debug, Serialize)]
#[serde(tag = "event")]
enum ControlPlaneResponse {
    #[serde(rename = "editorScrollTo")]
    EditorScrollTo(DocToSrcJumpInfo),
    #[serde(rename = "syncEditorChanges")]
    SyncEditorChanges(()),
    #[serde(rename = "compileStatus")]
    CompileStatus(CompileStatus),
    #[serde(rename = "outline")]
    Outline(Outline),
}

impl EditorActor {
    pub fn new(
        mailbox: mpsc::UnboundedReceiver<EditorActorRequest>,
        editor_websocket_conn: WebSocketStream<TcpStream>,
        world_sender: mpsc::UnboundedSender<TypstActorRequest>,
        webview_sender: broadcast::Sender<WebviewActorRequest>,
        span_interner: Arc<RwLock<SpanInterner>>,
    ) -> Self {
        Self {
            mailbox,
            editor_websocket_conn,
            world_sender,
            webview_sender,

            span_interner,
        }
    }

    pub async fn run(mut self) {
        self.editor_websocket_conn
            .send(Message::Text(
                serde_json::to_string(&ControlPlaneResponse::SyncEditorChanges(())).unwrap(),
            ))
            .await
            .unwrap();
        loop {
            tokio::select! {
                Some(msg) = self.mailbox.recv() => {
                    trace!("EditorActor: received message from mailbox: {:?}", msg);
                    match msg {
                        EditorActorRequest::DocToSrcJump(jump_info) => {
                            let Ok(_) = self.editor_websocket_conn.send(Message::Text(
                                serde_json::to_string(&ControlPlaneResponse::EditorScrollTo(jump_info)).unwrap(),
                            )).await else {
                                warn!("EditorActor: failed to send DocToSrcJump message to editor");
                                break;
                            };
                        },
                        EditorActorRequest::DocToSrcJumpResolve(req) => {
                            self.source_scroll_by_span(req.span).await;
                        },
                        EditorActorRequest::CompileStatus(status) => {
                            let Ok(_) = self.editor_websocket_conn.send(Message::Text(
                                serde_json::to_string(&ControlPlaneResponse::CompileStatus(status)).unwrap(),
                            )).await else {
                                warn!("EditorActor: failed to send CompileStatus message to editor");
                                break;
                            };
                        },
                        EditorActorRequest::Outline(outline) => {
                            let Ok(_) = self.editor_websocket_conn.send(Message::Text(
                                serde_json::to_string(&ControlPlaneResponse::Outline(outline)).unwrap(),
                            )).await else {
                                warn!("EditorActor: failed to send Outline message to editor");
                                break;
                            };
                        }
                    }
                }
                Some(Ok(Message::Text(msg))) = self.editor_websocket_conn.next() => {
                    let Ok(msg) = serde_json::from_str::<ControlPlaneMessage>(&msg) else {
                        warn!("failed to parse jump request: {:?}", msg);
                        continue;
                    };
                    match msg {
                        ControlPlaneMessage::ChangeCursorPosition(cursor_info) => {
                            debug!("EditorActor: received message from editor: {:?}", cursor_info);
                            self.world_sender.send(TypstActorRequest::ChangeCursorPosition(cursor_info)).unwrap();
                        }
                        ControlPlaneMessage::SrcToDocJump(jump_info) => {
                            debug!("EditorActor: received message from editor: {:?}", jump_info);
                            self.world_sender.send(TypstActorRequest::SrcToDocJumpResolve(jump_info)).unwrap();
                        }
                        ControlPlaneMessage::PanelScrollByPosition(jump_info) => {
                            debug!("EditorActor: received message from editor: {:?}", jump_info);
                            self.webview_sender.send(WebviewActorRequest::ViewportPosition(jump_info.position)).unwrap();
                        }
                        ControlPlaneMessage::DocToSrcJumpResolve(jump_info) => {
                            debug!("EditorActor: received message from editor: {:?}", jump_info);

                            self.source_scroll_by_span(jump_info.span).await;
                        }
                        ControlPlaneMessage::SyncMemoryFiles(memory_files) => {
                            debug!("EditorActor: received message from editor: SyncMemoryFiles {:?}", memory_files.files.keys().collect::<Vec<_>>());
                            self.world_sender.send(TypstActorRequest::SyncMemoryFiles(memory_files)).unwrap();
                        }
                        ControlPlaneMessage::UpdateMemoryFiles(memory_files) => {
                            debug!("EditorActor: received message from editor: UpdateMemoryFiles {:?}", memory_files.files.keys().collect::<Vec<_>>());
                            self.world_sender.send(TypstActorRequest::UpdateMemoryFiles(memory_files)).unwrap();
                        }
                        ControlPlaneMessage::RemoveMemoryFiles(memory_files) => {
                            debug!("EditorActor: received message from editor: RemoveMemoryFiles {:?}", &memory_files.files);
                            self.world_sender.send(TypstActorRequest::RemoveMemoryFiles(memory_files)).unwrap();
                        }
                    };
                }
            }
        }
        info!("EditorActor: ws disconnected, shutting down whole program");
        std::process::exit(0);
    }

    async fn source_scroll_by_span(&mut self, span: String) {
        let jump_info = {
            let span_interner = self.span_interner.read().unwrap();
            match span_interner.span_by_str(&span) {
                InternQuery::Ok(s) => s.copied(),
                InternQuery::UseAfterFree => {
                    warn!("EditorActor: out of date span id: {}", span);
                    return;
                }
            }
        };
        if let Some(span) = jump_info {
            let span_and_offset = span.into();
            self.world_sender
                .send(TypstActorRequest::DocToSrcJumpResolve((
                    span_and_offset,
                    span_and_offset,
                )))
                .unwrap();
        };
    }
}
