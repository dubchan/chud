use super::{
	super::{
		rpc::cmd::{Cmd, CmdResp, LoadMsgReq, SubmitMsgReq},
		sys::{
			msg::{Message, MessageData},
			rt::Rt,
		},
		util::nonfatal,
	},
	behavior::{Behavior, BehaviorEvent},
	msg::{ConsensusRule, Context as MsgContext, Event as MsgEvent},
	sync::{Context as SyncContext, Error as SyncError, Event as SyncEvent},
	DB_NAME, NET_PROTOCOL_PREFIX, RR_PROTOCOL_PREFIX, RUNTIME_STORE, STATE_KEY,
	SYNCHRONIZATION_INTERVAL,
};
use async_channel::{Receiver, RecvError, Sender};
use futures::{future::FutureExt, select};
#[cfg(target_arch = "wasm32")]
use indexed_db_futures::{
	idb_transaction::IdbTransaction, prelude::IdbTransactionMode, request::IdbOpenDbRequestLike,
	IdbDatabase, IdbQuerySource, IdbVersionChangeEvent,
};
use libp2p::{
	core::{transport::Transport, upgrade::Version, ConnectedPoint},
	floodsub::Floodsub,
	futures::{Stream, StreamExt},
	identify::{Behaviour, Config},
	identity,
	kad::{record::store::MemoryStore, Kademlia, KademliaConfig, NoKnownPeers},
	multiaddr::{Error as MultiaddrError, Protocol},
	noise::{Config as NoiseConfig, Error as NoiseError},
	ping::Behaviour as PingBehavior,
	request_response::{cbor::Behaviour as RRBehavior, Config as RRConfig, ProtocolSupport},
	swarm::{
		keep_alive::Behaviour as KeepaliveBehavior, DialError, StreamProtocol, Swarm, SwarmBuilder,
		SwarmEvent,
	},
	Multiaddr, PeerId, TransportError,
};
use libp2p_autonat::{Behaviour as NATBehavior, Config as NATConfig};
use libp2p_mplex::MplexConfig;
use serde_wasm_bindgen::Error as SerdeWasmError;

use instant::Duration;
#[cfg(not(target_arch = "wasm32"))]
use libp2p::{
	dns::TokioDnsConfig,
	tcp::{tokio::Transport as TcpTransport, Config as TcpConfig},
	websocket::{
		tls::{Certificate, Config as TlsConfig, Error as TlsError, PrivateKey},
		WsConfig,
	},
};
#[cfg(target_arch = "wasm32")]
use libp2p_websys_transport::WebsocketTransport;
#[cfg(not(target_arch = "wasm32"))]
use openssl::{error::ErrorStack, pkcs12::Pkcs12};
use serde::{Deserialize, Serialize};
use std::{
	cfg,
	error::Error as StdError,
	fmt::{Display, Error as FmtError, Formatter},
	io::Error as IoError,
	net::Ipv4Addr,
};
#[cfg(not(target_arch = "wasm32"))]
use std::{fs::File as StdFile, io::Read};
use wasm_timer::Interval;

use wasm_bindgen::JsValue;

#[cfg(target_arch = "wasm32")]
use web_sys::DomException;

#[cfg(not(target_arch = "wasm32"))]
use tokio::{
	fs::File,
	io::{AsyncReadExt, AsyncWriteExt, Error as TokioError, Result as TokioResult},
};

use std::io::ErrorKind;

/// An error that could be encountered by the client.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub enum Error {
	NoiseError(NoiseError),
	MultiaddrError(MultiaddrError),
	NoKnownPeers,
	DialError(DialError),
	TransportError(TransportError<IoError>),
	SyncError(SyncError),
	SerdeWasmError(SerdeWasmError),
	RecvError(RecvError),
	TlsError(TlsError),
	IoError(IoError),
	OpenSslError(ErrorStack),
	MissingTlsKey,
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub enum Error {
	NoiseError(NoiseError),
	MultiaddrError(MultiaddrError),
	NoKnownPeers,
	DialError(DialError),
	TransportError(TransportError<IoError>),
	SyncError(SyncError),
	SerdeWasmError(SerdeWasmError),
	RecvError(RecvError),
	IoError(IoError),
}

impl Into<JsValue> for Error {
	fn into(self) -> JsValue {
		JsValue::from_str(self.to_string().as_str())
	}
}

impl From<NoiseError> for Error {
	fn from(e: NoiseError) -> Self {
		Self::NoiseError(e)
	}
}

impl From<MultiaddrError> for Error {
	fn from(e: MultiaddrError) -> Self {
		Self::MultiaddrError(e)
	}
}

impl From<NoKnownPeers> for Error {
	fn from(_: NoKnownPeers) -> Self {
		Self::NoKnownPeers
	}
}

impl From<DialError> for Error {
	fn from(e: DialError) -> Self {
		Self::DialError(e)
	}
}

impl From<TransportError<IoError>> for Error {
	fn from(e: TransportError<IoError>) -> Self {
		Self::TransportError(e)
	}
}

impl From<SyncError> for Error {
	fn from(e: SyncError) -> Self {
		Self::SyncError(e)
	}
}

impl From<SerdeWasmError> for Error {
	fn from(e: SerdeWasmError) -> Self {
		Self::SerdeWasmError(e)
	}
}

impl From<RecvError> for Error {
	fn from(e: RecvError) -> Self {
		Self::RecvError(e)
	}
}

#[cfg(not(target_arch = "wasm32"))]
impl From<TlsError> for Error {
	fn from(e: TlsError) -> Self {
		Self::TlsError(e)
	}
}

impl From<IoError> for Error {
	fn from(e: IoError) -> Self {
		Self::IoError(e)
	}
}

#[cfg(not(target_arch = "wasm32"))]
impl From<ErrorStack> for Error {
	fn from(e: ErrorStack) -> Self {
		Self::OpenSslError(e)
	}
}

#[cfg(not(target_arch = "wasm32"))]
impl StdError for Error {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		match self {
			Self::NoiseError(e) => Some(e),
			Self::MultiaddrError(e) => Some(e),
			Self::NoKnownPeers => Some(&NoKnownPeers()),
			Self::DialError(e) => Some(e),
			Self::TransportError(e) => Some(e),
			Self::SyncError(e) => Some(e),
			Self::SerdeWasmError(e) => Some(e),
			Self::RecvError(e) => Some(e),
			Self::TlsError(e) => Some(e),
			Self::IoError(e) => Some(e),
			Self::OpenSslError(e) => Some(e),
			Self::MissingTlsKey => None,
		}
	}
}

#[cfg(target_arch = "wasm32")]
impl StdError for Error {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		match self {
			Self::NoiseError(e) => Some(e),
			Self::MultiaddrError(e) => Some(e),
			Self::NoKnownPeers => Some(&NoKnownPeers()),
			Self::DialError(e) => Some(e),
			Self::TransportError(e) => Some(e),
			Self::SyncError(e) => Some(e),
			Self::SerdeWasmError(e) => Some(e),
			Self::RecvError(e) => Some(e),
			Self::IoError(e) => Some(e),
		}
	}
}

impl Display for Error {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
		match self.source() {
			Some(e) => write!(f, "Client encountered an error: {}", e),
			None => write!(f, "Client encountered an error"),
		}
	}
}

/// An interface with the CHUD network.
#[derive(Serialize, Deserialize)]
pub struct Client {
	pub runtime: Rt,
	chain_id: usize,

	// State variables
	bootstrapped: bool,

	// Pseudo-network behaviors
	#[serde(skip_serializing, skip_deserializing)]
	sync_context: SyncContext,

	#[serde(skip_serializing, skip_deserializing)]
	msg_context: MsgContext,
}

impl Client {
	/// Creates a new runtime with the given chain ID.
	pub fn new(chain_id: usize) -> Self {
		Self {
			chain_id,
			runtime: Rt::default(),
			bootstrapped: false,
			sync_context: SyncContext::default(),
			msg_context: MsgContext::default(),
		}
	}

	/// Adds a consensus rule to the message daemon.
	pub fn with_consensus_rule(&mut self, rule: ConsensusRule) {
		let rules = &mut self.msg_context.consensus_rules;
		rules.push(rule);
	}

	/// Loads the saved blockchain data from indexeddb.
	#[cfg(target_arch = "wasm32")]
	pub async fn load_from_disk(chain_id: usize) -> Result<Self, DomException> {
		let mut client = Client::new(chain_id);

		// Create the object store if it doesn't exist
		let mut db_req = IdbDatabase::open(DB_NAME)?;
		db_req.set_on_upgrade_needed(Some(|e: &IdbVersionChangeEvent| -> Result<(), JsValue> {
			if let None = e.db().object_store_names().find(|n| n == RUNTIME_STORE) {
				e.db().create_object_store(RUNTIME_STORE)?;
			}

			Ok(())
		}));

		// Open the database
		let db: IdbDatabase = db_req.into_future().await?;

		// Read the state from the database
		let tx: IdbTransaction =
			db.transaction_on_one_with_mode(RUNTIME_STORE, IdbTransactionMode::Readonly)?;
		let store = tx.object_store(RUNTIME_STORE)?;

		if let Some(Ok(record)) = store.get_owned(STATE_KEY)?.await.and_then(|val| {
			Ok(val.map(|val| {
				serde_wasm_bindgen::from_value(val)
					.map_err(|e| DomException::from(JsValue::from_str(format!("{}", e).as_str())))
			}))
		})? {
			client = record;
		}

		Ok(client)
	}

	/// Loads the saved blockchain data from a JSON file
	#[cfg(not(target_arch = "wasm32"))]
	pub async fn load_from_disk(chain_id: usize) -> TokioResult<Self> {
		let mut client = Client::new(chain_id);

		// Read the entire database file, and then deserialize it
		if let Ok(mut f) = File::open(DB_NAME).await {
			let mut contents = Vec::new();
			f.read_to_end(&mut contents).await?;

			client = serde_json::from_slice(contents.as_slice())
				.map_err(|e| TokioError::new(ErrorKind::InvalidData, e))?;
		};

		Ok(client)
	}

	/// Writes the blockchain to indexeddb.
	#[cfg(target_arch = "wasm32")]
	pub async fn write_to_disk(&self) -> Result<(), DomException> {
		// Create the object store if it doesn't exist
		let mut db_req = IdbDatabase::open(DB_NAME)?;
		db_req.set_on_upgrade_needed(Some(|e: &IdbVersionChangeEvent| -> Result<(), JsValue> {
			if let None = e.db().object_store_names().find(|n| n == RUNTIME_STORE) {
				e.db().create_object_store(RUNTIME_STORE)?;
			}

			Ok(())
		}));

		// Open the database
		let db: IdbDatabase = db_req.into_future().await?;

		// Write the state to the database
		let tx: IdbTransaction =
			db.transaction_on_one_with_mode(RUNTIME_STORE, IdbTransactionMode::Readwrite)?;
		let store = tx.object_store(RUNTIME_STORE)?;

		store.put_key_val_owned(
			STATE_KEY,
			&serde_wasm_bindgen::to_value(self)
				.map_err(|e| DomException::from(JsValue::from_str(format!("{}", e).as_str())))?,
		)?;

		Ok(())
	}

	/// Saves the blockchain to a database file in JSON format.
	#[cfg(not(target_arch = "wasm32"))]
	pub async fn write_to_disk(&self) -> TokioResult<()> {
		// Open the database file and write the serialized blockchain to it
		let mut f = File::create(DB_NAME).await?;

		let ser =
			serde_json::to_vec(self).map_err(|e| TokioError::new(ErrorKind::InvalidData, e))?;
		f.write_all(ser.as_slice()).await?;

		Ok(())
	}

	#[cfg(target_arch = "wasm32")]
	fn build_swarm(&self, cert_path: Option<String>) -> Result<Swarm<Behavior>, Error> {
		// Use WebSockets as a transport.
		// TODO: Use webrtc in the future for p2p in browsers
		let local_key = identity::Keypair::generate_ed25519();
		let local_peer_id = PeerId::from(local_key.public());

		let transport = WebsocketTransport::default()
			.upgrade(Version::V1Lazy)
			.authenticate(NoiseConfig::new(&local_key)?)
			.multiplex(MplexConfig::default())
			.boxed();

		// Create a swarm with the desired behavior
		{
			let store = MemoryStore::new(local_peer_id);
			let mut kad_conf = KademliaConfig::default();
			kad_conf.set_max_packet_size(30 * 1024);
			let kad = Kademlia::with_config(local_peer_id, store, kad_conf);
			let floodsub = Floodsub::new(local_peer_id);
			let identify = Behaviour::new(Config::new(
				format!("{}{}", NET_PROTOCOL_PREFIX, self.chain_id),
				local_key.public(),
			));
			let rresponse = RRBehavior::new(
				[(
					StreamProtocol::new(RR_PROTOCOL_PREFIX),
					ProtocolSupport::Full,
				)],
				RRConfig::default(),
			);
			let ping = PingBehavior::default();
			let keep_alive = KeepaliveBehavior::default();
			let autonat = NATBehavior::new(local_peer_id, NATConfig::default());

			Ok(SwarmBuilder::with_wasm_executor(
				transport,
				Behavior::new(
					kad, floodsub, identify, rresponse, ping, keep_alive, autonat,
				),
				local_peer_id,
			)
			.build())
		}
	}

	#[cfg(not(target_arch = "wasm32"))]
	fn build_swarm(&self, cert_path: Option<String>) -> Result<Swarm<Behavior>, Error> {
		// Use WebSockets as a transport.
		// TODO: Use webrtc in the future for p2p in browsers
		let local_key = identity::Keypair::generate_ed25519();
		let local_peer_id = PeerId::from(local_key.public());

		let mut conf = WsConfig::new(TokioDnsConfig::system(TcpTransport::new(TcpConfig::new()))?);
		if let Some(cert_path) = cert_path {
			let mut b = Vec::new();
			let mut f = StdFile::open(cert_path)?;
			f.read_to_end(&mut b)?;

			let data = Pkcs12::from_der(b.as_slice())?;
			let parsed = data.parse2("")?;
			let priv_key = PrivateKey::new(
				parsed
					.pkey
					.ok_or(Error::MissingTlsKey)?
					.private_key_to_der()?,
			);
			let cert = Certificate::new(parsed.cert.ok_or(Error::MissingTlsKey)?.to_der()?);

			conf.set_tls_config(TlsConfig::new(priv_key, vec![cert])?);
		}

		let transport = conf
			.upgrade(Version::V1Lazy)
			.authenticate(NoiseConfig::new(&local_key)?)
			.multiplex(MplexConfig::default())
			.boxed();

		// Create a swarm with the desired behavior
		{
			let store = MemoryStore::new(local_peer_id);
			let mut kad_conf = KademliaConfig::default();
			kad_conf.set_max_packet_size(30 * 1024);
			let kad = Kademlia::with_config(local_peer_id, store, kad_conf);
			let floodsub = Floodsub::new(local_peer_id);
			let identify = Behaviour::new(Config::new(
				format!("{}{}", NET_PROTOCOL_PREFIX, self.chain_id),
				local_key.public(),
			));
			let rresponse = RRBehavior::new(
				[(
					StreamProtocol::new(RR_PROTOCOL_PREFIX),
					ProtocolSupport::Full,
				)],
				RRConfig::default(),
			);
			let ping = PingBehavior::default();
			let keep_alive = KeepaliveBehavior::default();
			let autonat = NATBehavior::new(local_peer_id, NATConfig::default());

			Ok(SwarmBuilder::with_tokio_executor(
				transport,
				Behavior::new(
					kad, floodsub, identify, rresponse, ping, keep_alive, autonat,
				),
				local_peer_id,
			)
			.build())
		}
	}

	/// Synchronizes and keep sthe client in sync with the network. Accepts
	/// commands on a receiving channel for operations to perform.
	pub async fn start(
		mut self,
		mut cmd_rx: Receiver<Cmd>,
		resp_tx: Sender<CmdResp>,
		bootstrap_peers: Vec<String>,
		listen_port: Option<u16>,
		external_addresses: Vec<Multiaddr>,
		cert_path: Option<String>,
	) -> Result<(), Error> {
		let is_secure = cert_path.is_some();
		let mut swarm = self.build_swarm(cert_path)?;

		for external_addr in external_addresses {
			swarm.add_external_address(external_addr);
		}

		// Dial all bootstrap peers
		for multiaddr in bootstrap_peers.iter() {
			swarm
				.dial(
					multiaddr
						.parse::<Multiaddr>()
						.map_err(<MultiaddrError as Into<Error>>::into)?,
				)
				.map_err(<DialError as Into<Error>>::into)?;
		}

		if let Some(listen_port) = listen_port {
			// Listen for connections on the given port.
			let address = Multiaddr::from(Ipv4Addr::UNSPECIFIED)
				.with(Protocol::Tcp(listen_port))
				.with(if is_secure {
					Protocol::Wss("/".into())
				} else {
					Protocol::Ws("/".into())
				});
			swarm
				.listen_on(address.clone())
				.map_err(<TransportError<IoError> as Into<Error>>::into)?;

			info!("p2p client listening on {}", address);
		}

		// Write all transactions to the DHT and synchronize the chain
		// every n minutes
		let mut sync_fut = Interval::new(Duration::from_millis(SYNCHRONIZATION_INTERVAL)).fuse();

		loop {
			select! {
				event = swarm.select_next_some() => {
					match event {
					SwarmEvent::Behaviour(event) => {
						// Check if the sync context has something to say about this
						let (out_event, in_event) = self.sync_context.poll(&mut self.runtime, swarm.behaviour_mut().request_response_mut(), Some(event));
						match out_event {
							Ok(Some(e)) => match e {
								SyncEvent::MessageCommitted(h) => {
									info!("message {} successfully committed to the DHT", hex::encode(h));
								},
								SyncEvent::LongestChainUpdated { hash, .. } => {
									info!("got new longest chain {}", hex::encode(&hash));

									self.sync_context.download_msg(&hash, swarm.behaviour_mut().kad_mut())?;
								},
								SyncEvent::MessageLoaded(msg) => {
									info!("message {} loaded", hex::encode(msg.hash()));

									// Download the message if it doesn't exist locally
									if let Some(prev) = msg.data().prev() {
										if self.runtime.get_message(prev).is_none() {
											self.sync_context.download_msg(&prev, swarm.behaviour_mut().kad_mut())?;
										}
									}
								},
								SyncEvent::MessageLoadCompleted{ msg, req_id } => {
									nonfatal!(resp_tx.send(CmdResp::MsgLoaded { msg, req_id }).await, req_id, resp_tx);
								},
								SyncEvent::MessageLoadFailed { req_id } => {
									error!("failed to load message");

									nonfatal!(resp_tx.send(CmdResp::Error { error: "Failed to load the message.".into(), req_id}).await, req_id, resp_tx);
								}
							},
							Err(e) => error!("synchronization failed: {}", e),
							_ => {},
						}

						// Check if the message context has something to say about this
						let (out_event, _) = self.msg_context.poll(&mut self.runtime, swarm.behaviour_mut().floodsub_mut(), in_event);
						match out_event {
							Ok(Some(e)) => match e {
								MsgEvent::MessageReceived(h) => {
									info!("Message received: {}", hex::encode(h));
								}
							},
							Err(e) => error!("message handling failed: {}", e),
							_ => {},
						}
					},
					SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
						// Register peers in the kademlia DHT and floodsub once they're found
						match endpoint {
							ConnectedPoint::Dialer {
								address, ..
							} => {
								swarm.behaviour_mut().kad_mut().add_address(&peer_id, address.clone());
								swarm.behaviour_mut().request_response_mut().add_address(&peer_id, address.clone());

								// Bootstrap the DHT if we connected to one of the bootstrap addresses
								if !self.bootstrapped && bootstrap_peers.contains(&address.to_string()) {
									let sampling_pool = swarm.connected_peers().map(|x| x.clone()).collect::<Vec<PeerId>>();

									swarm.behaviour_mut().kad_mut().bootstrap().map_err(<NoKnownPeers as Into<Error>>::into)?;
									if let Err(e) = self
										.sync_context
										.download_head(swarm.behaviour_mut().request_response_mut(), sampling_pool.iter().collect::<Vec<&PeerId>>())
									{
										error!("Failed to download chain: {}", e);
									};

									if let Err(e) = self
										.sync_context
										.upload_chain(&self.runtime, swarm.behaviour_mut().kad_mut())
									{
										error!("Failed to upload chain: {}", e);
									};

									self.bootstrapped = true;

									info!("successfully bootstrapped to peer {}", &peer_id);
								}

							},
							_ => {}
						}

						swarm.behaviour_mut().floodsub_mut().add_node_to_partial_view(peer_id);
					},
					SwarmEvent::ConnectionClosed { peer_id, endpoint, .. } => {
						// Remove disconnected peers
						swarm.behaviour_mut().kad_mut().remove_peer(&peer_id);
						swarm.behaviour_mut().floodsub_mut().remove_node_from_partial_view(&peer_id);

						// Remove the request-response peer
						if let ConnectedPoint::Dialer { address, .. } = endpoint {
							swarm.behaviour_mut().request_response_mut().remove_address(&peer_id, &address);
						}
					}
					_ => {}
				}},
				cmd = cmd_rx.select_next_some() => match cmd {
					Cmd::Terminate => break Ok(()),
					Cmd::SubmitMsg{ req: SubmitMsgReq{ data, prev, captcha_ans,captcha_src, height, timestamp}, req_id } => {
						let msg = nonfatal!(Message::try_from(MessageData::new(data, prev, captcha_ans, captcha_src, height, timestamp)), req_id, resp_tx);
						let hash = msg.hash().clone();
						match self.msg_context.submit_message(&mut self.runtime, msg, swarm.behaviour_mut().floodsub_mut()) {
							Ok(_) => {
								nonfatal!(resp_tx.send(CmdResp::MsgSubmitted{ hash, req_id }).await, req_id, resp_tx);
							},
							Err(e) => error!("Failed to submit message {}: {}", hex::encode(hash), e),
						}
					},
					Cmd::LoadMsg { req: LoadMsgReq { hash }, req_id } => {
						// If the message exists locally, just use that
						if let Some(msg) = self.runtime.get_message(&hash) {
							nonfatal!(resp_tx.send(CmdResp::MsgLoaded { msg: msg.clone(), req_id }).await, req_id, resp_tx);
							continue;
						}

						// Otherwise, download it
						nonfatal!(self.sync_context.load_msg(&hash, swarm.behaviour_mut().kad_mut(), req_id), req_id, resp_tx);
					},
					Cmd::GetHead { req_id } => {
						if let Some(head) = self.runtime.longest_chain() {
							nonfatal!(resp_tx.send(CmdResp::HeadLoaded { hash: head.clone(), req_id }).await, req_id, resp_tx);
							continue;
						}

						nonfatal!(resp_tx.send(CmdResp::Error{ error: String::from("Missing chain HEAD."), req_id}).await, req_id, resp_tx);
					}
				},
				_ = sync_fut.next() => {
					if let Err(e) = self.sync_context.upload_chain(&self.runtime, swarm.behaviour_mut().kad_mut()) {
						error!("Failed to upload chain: {}", e);
					}
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::super::super::sys::msg::{Message, MessageData};
	use super::*;

	#[cfg(not(target_arch = "wasm32"))]
	use tokio::task::JoinError;

	#[test]
	fn test_new() {
		let client = Client::new(0);
		assert_eq!(client.chain_id, 0);
	}

	#[cfg(not(target_arch = "wasm32"))]
	#[tokio::test]
	async fn test_write_load() -> Result<(), Box<dyn StdError>> {
		let mut client = Client::new(0);

		let data = MessageData::new(Vec::new(), None, None, None, 0, 0);
		let msg = Message::try_from(data)?;

		// Insert the message
		client.runtime.insert_message(msg);

		// Ensure read and written clients are the same
		client.write_to_disk().await?;
		let client2 = Client::load_from_disk(0).await?;

		assert_eq!(client.runtime, client2.runtime);

		Ok(())
	}

	#[cfg(not(target_arch = "wasm32"))]
	#[tokio::test]
	async fn test_start() -> Result<(), Box<dyn StdError>> {
		let (tx, rx) = async_channel::unbounded();
		let (tx_resp, _) = async_channel::unbounded();
		tx.send(Cmd::Terminate).await?;

		let client = Client::new(0);
		client
			.start(rx, tx_resp, Vec::new(), Some(6224), Vec::new(), None)
			.await?;

		Ok(())
	}

	#[cfg(not(target_arch = "wasm32"))]
	#[tokio::test]
	async fn test_submit_message() -> Result<(), Box<dyn StdError>> {
		let (tx, rx) = async_channel::unbounded();
		let (tx_resp, rx_resp) = async_channel::unbounded();

		let client = Client::new(0);
		let join = tokio::spawn(async {
			client
				.start(rx, tx_resp, Vec::new(), Some(6224), Vec::new(), None)
				.await
				.map_err(|e| e.to_string())
		});

		tx.send(Cmd::SubmitMsg {
			req: SubmitMsgReq {
				data: Vec::new(),
				prev: None,
				captcha_ans: None,
				captcha_src: None,
				height: 0,
				timestamp: 0,
			},
			req_id: 0,
		})
		.await?;
		let resp = rx_resp.recv().await?;
		tx.send(Cmd::Terminate).await?;

		assert!(matches!(resp, CmdResp::MsgSubmitted { .. }));

		join.await
			.map_err(|e| <JoinError as Into<Box<dyn StdError>>>::into(e))?
			.map_err(|e| e.into())
	}

	#[cfg(not(target_arch = "wasm32"))]
	#[tokio::test]
	async fn test_load_message() -> Result<(), Box<dyn StdError>> {
		let (tx, rx) = async_channel::unbounded();
		let (tx_resp, rx_resp) = async_channel::unbounded();

		let client = Client::new(0);
		let join = tokio::spawn(async {
			client
				.start(rx, tx_resp, Vec::new(), Some(6224), Vec::new(), None)
				.await
				.map_err(|e| e.to_string())
		});

		tx.send(Cmd::SubmitMsg {
			req: SubmitMsgReq {
				data: Vec::new(),
				prev: None,
				captcha_ans: None,
				captcha_src: None,
				height: 0,
				timestamp: 0,
			},
			req_id: 0,
		})
		.await?;
		let resp = rx_resp.recv().await?;

		assert!(matches!(resp, CmdResp::MsgSubmitted { .. }));

		let hash = match resp {
			CmdResp::MsgSubmitted { hash, .. } => hash,
			_ => {
				panic!("Invalid response. Expected hash.");
			}
		};

		// Ensure that the found message has the same data that we put in
		tx.send(Cmd::LoadMsg {
			req: LoadMsgReq { hash: hash.clone() },
			req_id: 1,
		})
		.await?;
		let resp = rx_resp.recv().await?;

		assert!(matches!(resp, CmdResp::MsgLoaded { .. }));

		match resp {
			CmdResp::MsgLoaded { msg, .. } => {
				assert_eq!(msg.hash(), &hash);
			}
			_ => {
				panic!("Invalid response. Expected message.");
			}
		}

		tx.send(Cmd::Terminate).await?;

		join.await
			.map_err(|e| <JoinError as Into<Box<dyn StdError>>>::into(e))?
			.map_err(|e| e.into())
	}
}
