use std::time::Duration;

use crossterm::event::{EventStream, KeyEvent};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app::UsageData;

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    Render,
    UsageResult {
        account_name: String,
        result: Result<UsageData, String>,
    },
    OAuthImportResult {
        result: Result<OAuthImportData, String>,
    },
    Resize(u16, u16),
}

#[derive(Debug)]
pub struct OAuthImportData {
    pub name: String,
    pub org_id: String,
    pub credential_json: String,
}

pub struct EventHandler {
    tx: mpsc::UnboundedSender<Event>,
    rx: mpsc::UnboundedReceiver<Event>,
    _task: JoinHandle<()>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration, render_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let sender = tx.clone();
        let task = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut tick_interval = tokio::time::interval(tick_rate);
            let mut render_interval = tokio::time::interval(render_rate);
            loop {
                tokio::select! {
                    Some(Ok(evt)) = reader.next() => {
                        match evt {
                            crossterm::event::Event::Key(key) => {
                                let _ = sender.send(Event::Key(key));
                            }
                            crossterm::event::Event::Resize(w, h) => {
                                let _ = sender.send(Event::Resize(w, h));
                            }
                            _ => {}
                        }
                    }
                    _ = tick_interval.tick() => {
                        let _ = sender.send(Event::Tick);
                    }
                    _ = render_interval.tick() => {
                        let _ = sender.send(Event::Render);
                    }
                }
            }
        });
        Self {
            tx,
            rx,
            _task: task,
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}
