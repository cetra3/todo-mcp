use automerge::AutoCommit;

use anyhow::Context;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use tracing::*;

use tokio::sync::mpsc::{
    Receiver as TokioReceiver, Sender as TokioSender, channel as tokio_channel,
};

use tokio::sync::oneshot::{Sender as OneshotSender, channel as oneshot_channel};

use tokio::sync::{Notify, RwLock};

use autosurgeon::{Hydrate, Reconcile, hydrate, reconcile};
use std::sync::Arc;

use iroh::EndpointId;

use crate::backends::ipc;
use crate::backends::iroh as iroh_transport;
use crate::backends::proto::{McastReceiver, McastSender, ProtoMessage};

#[cfg(target_os = "android")]
const STORAGE_LOCATION: &str = "/data/data/dev.cetra.todomcp/files/automerge.save";

#[cfg(not(target_os = "android"))]
const STORAGE_LOCATION: &str = "~/.local/share/todo_mcp/automerge.save";

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub completed: bool,
    #[serde(default)]
    #[autosurgeon(missing = "Default::default")]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Reconcile, Hydrate, PartialEq, Serialize, Deserialize)]
pub struct TodoList {
    pub title: String,
    pub items: Vec<TodoItem>,
    #[serde(default)]
    #[autosurgeon(missing = "Default::default")]
    pub metadata: HashMap<String, String>,
}

impl TodoList {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TodoEvent {
    StateUpdate(TodoState),
    ConnectionStatus(String),
}

#[derive(Debug, Default, Clone, Reconcile, Hydrate, PartialEq, Serialize, Deserialize)]
pub struct TodoState {
    pub lists: Vec<TodoList>,
}

#[derive(Debug)]
pub enum TodoCommand {
    // List operations
    AddList {
        title: String,
        metadata: HashMap<String, String>,
    },
    RemoveList {
        list_index: usize,
    },
    RenameList {
        list_index: usize,
        title: String,
    },

    // Item operations (now require list_index)
    AddTodo {
        list_index: usize,
        text: String,
        metadata: HashMap<String, String>,
    },
    RenameTodo {
        list_index: usize,
        item_index: usize,
        text: String,
    },
    ToggleTodo {
        list_index: usize,
        item_index: usize,
    },
    RemoveTodo {
        list_index: usize,
        item_index: usize,
    },
    ClearCompleted {
        list_index: usize,
    },

    // Sync operations
    Shutdown {
        sender: OneshotSender<()>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SyncMessage {
    DeltaChange(Vec<u8>),
    State(Vec<u8>),
    RequestState(u32),
    Announce(Vec<u8>),
    Alive,
    Shutdown,
}

pub fn setup(site_id: u32) -> (TokioSender<TodoCommand>, TokioReceiver<TodoEvent>) {
    // a few channels to setup

    // change coming in from one of our clients
    let (change_tx, change_rx) = tokio_channel(128);

    // messages received from multicast
    let (message_tx, message_rx) = tokio_channel(128);

    tokio::spawn(async move {
        if let Err(err) = async_inner(site_id, message_tx, change_rx).await {
            error!("Error with async task:{err:?}");
        };
    });

    (change_tx, message_rx)
}

pub struct SiteState {
    pub(crate) commit: AutoCommit,
    alive: BTreeMap<u32, Instant>,
}

const ALIVE_TIMEOUT: Duration = Duration::from_secs(5);

impl SiteState {
    async fn new(file_location: PathBuf, change_tx: TokioSender<TodoEvent>) -> Result<Self> {
        let file_data = {
            tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
                match std::fs::File::open(&file_location) {
                    Ok(file) => {
                        let mut data = Vec::new();
                        std::io::Read::read_to_end(&mut &file, &mut data)?;
                        Ok(Some(data))
                    }
                    Err(_) => Ok(None),
                }
            })
            .await?
        }?;

        let commit = if let Some(file) = file_data {
            let autocommit = AutoCommit::load(&file)?;
            let state = hydrate(&autocommit)?;
            change_tx.send(TodoEvent::StateUpdate(state)).await?;
            autocommit
        } else {
            debug!("creating new file");
            let mut autocommit = AutoCommit::new();
            let state: TodoState = TodoState::default();
            reconcile(&mut autocommit, state.clone())?;
            change_tx.send(TodoEvent::StateUpdate(state)).await?;
            autocommit
        };

        Ok(Self {
            commit,
            alive: BTreeMap::new(),
        })
    }

    async fn update_aliveness(
        &mut self,
        tx: &TokioSender<TodoEvent>,
        incoming_site_id: Option<u32>,
    ) -> Result<()> {
        let alive_count_before = self.alive.len();

        if let Some(id) = incoming_site_id {
            self.alive.insert(id, Instant::now());
        }
        self.alive.retain(|_, time| time.elapsed() < ALIVE_TIMEOUT);

        let alive_count_after = self.alive.len();

        if alive_count_after != alive_count_before {
            debug!(
                "Alive count changed from {} to {}",
                alive_count_before, alive_count_after
            );

            tx.send(TodoEvent::ConnectionStatus(format!(
                "Connections: {}",
                alive_count_after
            )))
            .await?;
        }

        Ok(())
    }

    async fn merge(&mut self, val: Vec<u8>) -> Result<()> {
        let mut other = AutoCommit::load(&val)?;

        let current_value: TodoState = hydrate(&self.commit)?;

        // If this is a newly initialised instance
        if current_value.lists.is_empty() {
            debug!("Pristine state, replacing ours with theirs");
            let new = other.with_actor(self.commit.get_actor().clone());

            self.commit = new;
        } else {
            self.commit.merge(&mut other)?;
        }

        Ok(())
    }

    async fn shutdown_site(
        &mut self,
        tx: &TokioSender<TodoEvent>,
        incoming_site_id: u32,
    ) -> Result<()> {
        self.alive.remove(&incoming_site_id);
        tx.send(TodoEvent::ConnectionStatus(format!(
            "Site Disconnected, Connections: {}",
            self.alive.len()
        )))
        .await?;
        Ok(())
    }
}

pub type Site = Arc<RwLock<SiteState>>;

/// Multicast announcement message — only broadcasts EndpointId for LAN discovery.
/// All actual data sync goes through iroh QUIC streams.
#[derive(Serialize, Deserialize, Debug)]
pub enum AnnouncementMessage {
    IrohAnnounce([u8; 32]),
}

#[instrument(skip(change_tx, change_rx))]
pub async fn async_inner(
    site_id: u32,
    change_tx: TokioSender<TodoEvent>,
    change_rx: TokioReceiver<TodoCommand>,
) -> Result<()> {
    let file_location: PathBuf = shellexpand::tilde(
        &std::env::var("TODOMCP_AUTOSAVE_PATH").unwrap_or_else(|_| STORAGE_LOCATION.to_owned()),
    )
    .to_string()
    .into();

    let (file_write_tx, file_write_rx) = tokio_channel::<OneshotSender<()>>(128);
    let (m_write_tx, mut m_write_rx) = tokio_channel::<SyncMessage>(128);
    let (multi_write_tx, multi_write_rx) = tokio_channel::<ProtoMessage<SyncMessage>>(128);

    // Fan-out channels: outbound messages go to both iroh and IPC
    let (iroh_outbound_tx, iroh_outbound_rx) = tokio_channel::<SyncMessage>(128);
    let (ipc_outbound_tx, ipc_outbound_rx) = tokio_channel::<SyncMessage>(128);

    let network_notify = Arc::new(Notify::new());

    let site = Arc::new(RwLock::new(
        SiteState::new(file_location.clone(), change_tx.clone()).await?,
    ));

    let mut join_set = JoinSet::new();

    // Fan-out task: m_write_rx -> iroh + IPC
    join_set.spawn(async move {
        while let Some(msg) = m_write_rx.recv().await {
            iroh_outbound_tx.send(msg.clone()).await.ok();
            ipc_outbound_tx.send(msg).await.ok();
        }
        Ok(())
    });

    join_set.spawn(save_to_file_task(
        file_location.clone(),
        site.clone(),
        file_write_rx,
    ));

    join_set.spawn(network_watcher(network_notify.clone()));

    let m_write_tx_read = m_write_tx.clone();

    join_set.spawn(write_notify(
        site.clone(),
        change_rx,
        m_write_tx,
        file_write_tx.clone(),
    ));

    join_set.spawn(read_notify(
        site_id,
        site.clone(),
        multi_write_rx,
        change_tx.clone(),
        m_write_tx_read,
        file_write_tx.clone(),
    ));

    // Setup IPC for same-machine sync
    let storage_dir = file_location.parent().unwrap().to_path_buf();
    join_set.spawn(ipc::setup_ipc(
        site_id,
        storage_dir,
        site.clone(),
        multi_write_tx.clone(),
        ipc_outbound_rx,
    ));

    // Setup iroh — takes ownership of outbound and feeds inbound
    let iroh_handle = iroh_transport::setup_iroh(site.clone(), multi_write_tx.clone(), iroh_outbound_rx).await?;

    let our_endpoint_id = iroh_handle.our_endpoint_id;
    let discovered_peer_tx = iroh_handle.discovered_peer_tx;

    info!(
        "Iroh endpoint ready: {}",
        our_endpoint_id.fmt_short()
    );

    // Multicast is now announcement-only
    let mut read_task = read_from_multicast_announce(discovered_peer_tx.clone());
    let mut write_task = write_to_multicast_announce(site_id, our_endpoint_id);

    loop {
        tokio::select! {
            _ = network_notify.notified() => {
                debug!("network watcher notified");
                change_tx.send(TodoEvent::ConnectionStatus("Network status changed".into())).await?;
            }
            result = read_task => {
                if let Err(err) = result {
                    change_tx.send(TodoEvent::ConnectionStatus(format!("Error reading announcements: {err}, trying reconnect in 10s"))).await?;
                    error!("Error reading multicast announcements, sleeping 10s: {err:?}");
                }
            }
            result = write_task => {
                if let Err(err) = result {
                    change_tx.send(TodoEvent::ConnectionStatus(format!("Error writing announcements: {err}, trying reconnect in 10s"))).await?;
                    error!("Error writing multicast announcements, sleeping 10s: {err:?}");
                }
            }
        }

        let sleep = tokio::time::sleep(std::time::Duration::from_secs(10));

        tokio::select! {
            _ = sleep => {
                debug!("sleep finished, trying again");
            },
            _ = network_notify.notified() => {
                debug!("network watcher notified, trying again");
            }
        }

        change_tx
            .send(TodoEvent::ConnectionStatus("Reconnecting".into()))
            .await?;

        read_task = read_from_multicast_announce(discovered_peer_tx.clone());
        write_task = write_to_multicast_announce(site_id, our_endpoint_id);
    }
}

async fn network_watcher(notify: Arc<Notify>) -> Result<()> {
    let bg_notify = notify.clone();

    let mut skip_first_update = true;

    let _handle = netwatcher::watch_interfaces(move |_update| {
        debug!("We've received a network change");

        if skip_first_update {
            skip_first_update = false;
        } else {
            bg_notify.notify_waiters();
        }
    })?;

    loop {
        notify.notified().await;
    }
}

/// Saves the state to the file at the given location
async fn save_to_file_task(
    path_buf: PathBuf,
    site: Site,
    mut rx: TokioReceiver<OneshotSender<()>>,
) -> Result<()> {
    if let Some(parent) = path_buf.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "Could not create parent dir for writing: {}",
                parent.display()
            )
        })?;
    }

    while let Some(sender) = rx.recv().await {
        debug!("Save State Called");
        let state = {
            // we don't want to stuff up the save_incremental stuff so we save a clone
            site.read().await.commit.clone().save()
        };
        debug!("Grabbed State");

        tokio::fs::write(&path_buf, &state)
            .await
            .with_context(|| format!("could not save to path {}", path_buf.display()))?;

        debug!("Wrote State, notifying");
        // Notify the receiver that we have saved
        sender.send(()).ok();
    }

    Ok(())
}
// Reads packets from the multicast group and updates local state if necessary
#[instrument(skip_all, fields(site_id = site_id))]
pub async fn read_notify(
    site_id: u32,
    site: Site,
    mut m_read_rx: TokioReceiver<ProtoMessage<SyncMessage>>,
    change_tx: TokioSender<TodoEvent>,
    m_write_tx: TokioSender<SyncMessage>,
    write_tx: TokioSender<OneshotSender<()>>,
) -> Result<()> {
    let mut state_set = HashSet::new();
    let mut seen_deltas = HashSet::<u64>::new();
    let mut seen_deltas_order = VecDeque::<u64>::new();
    let mut join_set = JoinSet::new();

    let bg_site = site.clone();
    let bg_change_tx = change_tx.clone();

    // Update aliveness every second
    join_set.spawn(async move {
        loop {
            let mut announce_interval = tokio::time::interval(Duration::from_secs(5));

            announce_interval.tick().await;

            let mut wrt = bg_site.write().await;

            if let Err(err) = wrt.update_aliveness(&bg_change_tx, None).await {
                error!("Failed to update aliveness: {}", err);
                return;
            }
        }
    });

    while let Some(ProtoMessage {
        site_id: incoming_site_id,
        message,
    }) = m_read_rx.recv().await
    {
        // skip stuff from our site
        if incoming_site_id == site_id {
            continue;
        }

        let mut should_notify_save = false;

        match message {
            SyncMessage::DeltaChange(val) => {
                debug!("Site:{} DeltaChange:{}", incoming_site_id, val.len());
                // If we've seen this site before
                if state_set.contains(&incoming_site_id) {
                    // Hash-based dedup: skip if we've already processed these exact bytes
                    let delta_hash = {
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        val.hash(&mut hasher);
                        hasher.finish()
                    };
                    if seen_deltas.insert(delta_hash) {
                        seen_deltas_order.push_back(delta_hash);
                        if seen_deltas_order.len() > 2048 {
                            if let Some(old) = seen_deltas_order.pop_front() {
                                seen_deltas.remove(&old);
                            }
                        }
                        let mut wrt = site.write().await;
                        wrt.commit.load_incremental(&val)?;
                        let new_value: TodoState = hydrate(&wrt.commit)?;
                        should_notify_save = true;
                        change_tx.send(TodoEvent::StateUpdate(new_value)).await?;
                        // Re-broadcast to all peers via fan-out (iroh + IPC)
                        m_write_tx.send(SyncMessage::DeltaChange(val)).await?;
                    }
                } else {
                    // request the full state
                    m_write_tx
                        .send(SyncMessage::RequestState(incoming_site_id))
                        .await?;
                }
            }

            message @ (SyncMessage::State(_) | SyncMessage::Announce(_)) => {
                let is_announce = matches!(message, SyncMessage::Announce(_));

                let val = match message {
                    SyncMessage::State(val) => val,
                    SyncMessage::Announce(val) => val,
                    _ => unreachable!(),
                };

                if is_announce {
                    debug!(
                        "Announce from Site:{}, State:{}, sending our state",
                        incoming_site_id,
                        val.len()
                    );
                } else {
                    debug!("Site:{} State:{}", incoming_site_id, val.len());
                }
                let mut wrt = site.write().await;

                wrt.merge(val).await?;

                let new_value: TodoState = hydrate(&wrt.commit)?;

                change_tx.send(TodoEvent::StateUpdate(new_value)).await?;
                state_set.insert(incoming_site_id);

                should_notify_save = true;

                if is_announce {
                    m_write_tx
                        .send(SyncMessage::State(wrt.commit.save()))
                        .await?;
                }
            }
            SyncMessage::RequestState(requested_site_id) => {
                debug!(
                    "Site:{} RequestState:{}",
                    incoming_site_id, requested_site_id
                );
                if requested_site_id == site_id {
                    m_write_tx
                        .send(SyncMessage::State(site.write().await.commit.save()))
                        .await?;
                }
            }
            SyncMessage::Alive => {
                debug!("Alive from Site:{}", incoming_site_id);
                let mut wrt = site.write().await;

                wrt.update_aliveness(&change_tx, Some(incoming_site_id))
                    .await?;
            }
            SyncMessage::Shutdown => {
                debug!("Shutdown from Site:{}", incoming_site_id);
                let mut wrt = site.write().await;
                wrt.shutdown_site(&change_tx, incoming_site_id).await?;
            }
        }

        if should_notify_save {
            write_tx.try_send(oneshot_channel().0).ok();
        }
    }

    return Ok(());
}

/// Reads multicast announcements and forwards discovered EndpointIds to iroh.
#[instrument(skip_all)]
pub async fn read_from_multicast_announce(
    discovered_peer_tx: TokioSender<EndpointId>,
) -> Result<()> {
    let mut receiver = McastReceiver::<AnnouncementMessage>::new()?;

    while let Some(message) = receiver.try_next().await? {
        match message.message {
            AnnouncementMessage::IrohAnnounce(bytes) => {
                match iroh::PublicKey::from_bytes(&bytes) {
                    Ok(endpoint_id) => {
                        discovered_peer_tx.send(endpoint_id).await?;
                    }
                    Err(e) => {
                        warn!("Invalid endpoint ID in announcement: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}

#[instrument(skip_all)]
pub async fn write_notify(
    site: Site,
    mut change_rx: TokioReceiver<TodoCommand>,
    change_tx: TokioSender<SyncMessage>,
    write_tx: TokioSender<OneshotSender<()>>,
) -> Result<()> {
    while let Some(change) = change_rx.recv().await {
        let mut slock = site.write().await;
        let mut should_notify_save = false;

        let mut current_state: TodoState = hydrate(&slock.commit)?;

        let to_send = match change {
            // List operations
            TodoCommand::AddList { title, metadata } => {
                current_state.lists.push(TodoList {
                    title,
                    items: vec![],
                    metadata,
                });
                reconcile(&mut slock.commit, current_state)?;

                should_notify_save = true;
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::RemoveList { list_index } => {
                if list_index < current_state.lists.len() {
                    current_state.lists.remove(list_index);

                    reconcile(&mut slock.commit, current_state)?;

                    should_notify_save = true;
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::RenameList { list_index, title } => {
                if list_index < current_state.lists.len() {
                    current_state.lists[list_index].title = title;

                    reconcile(&mut slock.commit, current_state)?;

                    should_notify_save = true;
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }

            // Item operations
            TodoCommand::AddTodo {
                list_index,
                text,
                metadata,
            } => {
                if list_index < current_state.lists.len() {
                    current_state.lists[list_index].items.push(TodoItem {
                        text,
                        completed: false,
                        metadata,
                    });

                    reconcile(&mut slock.commit, current_state)?;

                    should_notify_save = true;
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::RenameTodo {
                list_index,
                item_index,
                text,
            } => {
                if list_index < current_state.lists.len() {
                    if let Some(item) = current_state.lists[list_index].items.get_mut(item_index) {
                        item.text = text;
                    }

                    reconcile(&mut slock.commit, current_state)?;

                    should_notify_save = true;
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::ToggleTodo {
                list_index,
                item_index,
            } => {
                if list_index < current_state.lists.len() {
                    let list = &mut current_state.lists[list_index];
                    if item_index < list.items.len() {
                        list.items[item_index].completed = !list.items[item_index].completed;

                        reconcile(&mut slock.commit, current_state)?;

                        should_notify_save = true;
                    }
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::RemoveTodo {
                list_index,
                item_index,
            } => {
                if list_index < current_state.lists.len() {
                    let list = &mut current_state.lists[list_index];
                    if item_index < list.items.len() {
                        list.items.remove(item_index);

                        reconcile(&mut slock.commit, current_state)?;

                        should_notify_save = true;
                    }
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }
            TodoCommand::ClearCompleted { list_index } => {
                if list_index < current_state.lists.len() {
                    current_state.lists[list_index]
                        .items
                        .retain(|item| !item.completed);

                    reconcile(&mut slock.commit, current_state)?;

                    should_notify_save = true;
                }
                SyncMessage::DeltaChange(slock.commit.save_incremental())
            }

            TodoCommand::Shutdown { sender } => {
                write_tx.send(sender).await.ok();
                SyncMessage::Shutdown
            }
        };

        if should_notify_save {
            write_tx.try_send(oneshot_channel().0).ok();
        }

        // We don't want to block this task if the network is down
        change_tx.try_send(to_send).ok();
    }
    Ok(())
}

/// Broadcasts our iroh EndpointId over multicast every 2 seconds for LAN discovery.
#[instrument(skip_all, fields(site_id = site_id))]
pub async fn write_to_multicast_announce(
    site_id: u32,
    endpoint_id: EndpointId,
) -> Result<()> {
    let mut mcast_sender = McastSender::new(site_id)?;
    let mut announce_interval = tokio::time::interval(Duration::from_secs(2));

    let announce_msg = AnnouncementMessage::IrohAnnounce(*endpoint_id.as_bytes());

    loop {
        announce_interval.tick().await;
        mcast_sender.send(&announce_msg).await?;
    }
}
