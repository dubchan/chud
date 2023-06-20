use super::{
	super::{
		crypto::hash::Hash,
		sys::{msg::Message, rt::Rt},
	},
	behavior::BehaviorEvent,
	FLOODSUB_MESSAGE_TOPIC,
};
use libp2p::floodsub::{Floodsub, FloodsubEvent, Topic};
use serde_json::Error as SerdeError;
use std::{
	error::Error as StdError,
	fmt::{Display, Error as FmtError, Formatter},
	time::{SystemTime, UNIX_EPOCH},
};

/// Events emitted by the message behavior
#[derive(Debug)]
pub enum Event {
	/// Emitted when a message gets received
	MessageReceived(Hash),
}

/// Errors emitted by the message behavior
#[derive(Debug)]
pub enum Error {
	SerializationError(SerdeError),
}

impl Display for Error {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
		match self {
			Self::SerializationError(e) => {
				write!(f, "encountered an error in serialization: {}", e)
			}
		}
	}
}

impl StdError for Error {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		match self {
			Self::SerializationError(e) => Some(e),
		}
	}
}

impl From<SerdeError> for Error {
	fn from(e: SerdeError) -> Self {
		Self::SerializationError(e)
	}
}

/// A context that handles swarm events dealing with messages.
#[derive(Default)]
pub struct Context;

impl Context {
	pub fn poll(
		&mut self,
		rt: &mut Rt,
		floodsub: &mut Floodsub,
		in_event: Option<BehaviorEvent>,
	) -> (Result<Option<Event>, Error>, Option<BehaviorEvent>) {
		match in_event {
			// Possible floodsub message topics:
			// - new_msg
			Some(BehaviorEvent::Floodsub(FloodsubEvent::Message(fs_msg))) => {
				// A new message has been received
				if fs_msg.topics.contains(&Topic::new(FLOODSUB_MESSAGE_TOPIC)) {
					if let Ok(msg) = serde_json::from_slice::<Message>(&fs_msg.data) {
						let hash = msg.hash().clone();

						if self.follows_consensus_rules(rt, &msg) {
							rt.insert_message(msg);

							info!(
								"Added message {} to the blockchain at height {}",
								hex::encode(msg.hash()),
								msg.data().height()
							);
						} else {
							error!("Rejecting message {}", hex::encode(msg.hash()));
						}

						return (Ok(Some(Event::MessageReceived(hash))), None);
					} else {
						(Ok(None), None)
					}
				} else {
					(Ok(None), None)
				}
			}
			_ => (Ok(None), in_event),
		}
	}

	/// Publishes a message to the floodsub messages topic.
	pub fn submit_message(&mut self, msg: Message, floodsub: &mut Floodsub) -> Result<(), Error> {
		let serialized = serde_json::to_vec(&msg)?;
		floodsub.publish(Topic::new(FLOODSUB_MESSAGE_TOPIC), serialized);

		Ok(())
	}

	/// Determines whether:
	/// - The hash of the message is valid
	/// - The timestamp of the message is valid
	/// - The message is at the front of the current longest_chain
	fn follows_consensus_rules(&self, rt: &Rt, msg: &Message) -> bool {
		rt.longest_chain()
			// Ensure prev is head
			.zip(msg.data().prev())
			.map(|(longest_chain, prev)| prev == longest_chain)
			// Ensure the message was made before now
			.and_then(|cond| {
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.map(|current_time| cond && msg.data().timestamp() < current_time.as_millis())
					.ok()
			})
			// Ensure the message was made after the head
			.and_then(|cond| {
				let longest_message = rt.get_message(rt.longest_chain()?)?;

				Some(cond && longest_message.data().timestamp() < msg.data().timestamp())
			})
			.unwrap_or_default()
	}
}