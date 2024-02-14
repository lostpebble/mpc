use super::message::PresignatureMessage;
use super::triple::{Triple, TripleId, TripleManager};
use crate::types::{PresignatureProtocol, PublicKey, SecretKeyShare};
use crate::util::AffinePointExt;
use cait_sith::protocol::{Action, InitializationError, Participant, ProtocolError};
use cait_sith::{KeygenOutput, PresignArguments, PresignOutput};
use k256::Secp256k1;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Unique number used to identify a specific ongoing presignature generation protocol.
/// Without `PresignatureId` it would be unclear where to route incoming cait-sith presignature
/// generation messages.
pub type PresignatureId = u64;

/// A completed presignature.
pub struct Presignature {
    pub id: PresignatureId,
    pub output: PresignOutput<Secp256k1>,
}

/// An ongoing presignature generator.
pub struct PresignatureGenerator {
    pub protocol: PresignatureProtocol,
    pub triple0: TripleId,
    pub triple1: TripleId,
    pub mine: bool,
    pub timestamp: Instant,
}

impl PresignatureGenerator {
    pub fn new(
        protocol: PresignatureProtocol,
        triple0: TripleId,
        triple1: TripleId,
        mine: bool,
    ) -> Self {
        Self {
            protocol,
            triple0,
            triple1,
            mine,
            timestamp: Instant::now(),
        }
    }

    pub fn poke(&mut self) -> Result<Action<PresignOutput<Secp256k1>>, ProtocolError> {
        if self.timestamp.elapsed() > crate::types::PROTOCOL_TIMEOUT {
            tracing::info!(
                self.triple0,
                self.triple1,
                self.mine,
                "presignature protocol timed out"
            );
            return Err(ProtocolError::Other(
                anyhow::anyhow!("presignature protocol timed out").into(),
            ));
        }

        self.protocol.poke()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("presignature already generated")]
    AlreadyGenerated,
    #[error("triple {0} is missing")]
    TripleIsMissing(TripleId),
    #[error("cait-sith initialization error: {0}")]
    CaitSithInitializationError(#[from] InitializationError),
}

/// Abstracts how triples are generated by providing a way to request a new triple that will be
/// complete some time in the future and a way to take an already generated triple.
pub struct PresignatureManager {
    /// Completed unspent presignatures.
    presignatures: HashMap<PresignatureId, Presignature>,
    /// Ongoing triple generation protocols.
    generators: HashMap<PresignatureId, PresignatureGenerator>,
    /// List of presignature ids generation of which was initiated by the current node.
    mine: VecDeque<PresignatureId>,

    participants: Vec<Participant>,
    me: Participant,
    threshold: usize,
    epoch: u64,
}

impl PresignatureManager {
    pub fn new(
        participants: Vec<Participant>,
        me: Participant,
        threshold: usize,
        epoch: u64,
    ) -> Self {
        Self {
            presignatures: HashMap::new(),
            generators: HashMap::new(),
            mine: VecDeque::new(),
            participants,
            me,
            threshold,
            epoch,
        }
    }

    /// Returns the number of unspent presignatures available in the manager.
    pub fn len(&self) -> usize {
        self.presignatures.len()
    }

    /// Returns the number of unspent presignatures assigned to this node.
    pub fn my_len(&self) -> usize {
        self.mine.len()
    }

    /// Returns the number of unspent presignatures we will have in the manager once
    /// all ongoing generation protocols complete.
    pub fn potential_len(&self) -> usize {
        self.presignatures.len() + self.generators.len()
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_internal(
        participants: &[Participant],
        me: Participant,
        threshold: usize,
        triple0: Triple,
        triple1: Triple,
        public_key: &PublicKey,
        private_share: &SecretKeyShare,
        mine: bool,
    ) -> Result<PresignatureGenerator, InitializationError> {
        let protocol = Box::new(cait_sith::presign(
            participants,
            me,
            PresignArguments {
                triple0: (triple0.share, triple0.public),
                triple1: (triple1.share, triple1.public),
                keygen_out: KeygenOutput {
                    private_share: *private_share,
                    public_key: *public_key,
                },
                threshold,
            },
        )?);
        Ok(PresignatureGenerator::new(
            protocol, triple0.id, triple1.id, mine,
        ))
    }

    /// Starts a new presignature generation protocol.
    pub fn generate(
        &mut self,
        triple0: Triple,
        triple1: Triple,
        public_key: &PublicKey,
        private_share: &SecretKeyShare,
    ) -> Result<(), InitializationError> {
        let id = rand::random();
        tracing::info!(id, "starting protocol to generate a new presignature");
        let generator = Self::generate_internal(
            &self.participants,
            self.me,
            self.threshold,
            triple0,
            triple1,
            public_key,
            private_share,
            true,
        )?;
        self.generators.insert(id, generator);
        Ok(())
    }

    /// Ensures that the presignature with the given id is either:
    /// 1) Already generated in which case returns `None`, or
    /// 2) Is currently being generated by `protocol` in which case returns `Some(protocol)`, or
    /// 3) Has never been seen by the manager in which case start a new protocol and returns `Some(protocol)`, or
    /// 4) Depends on triples (`triple0`/`triple1`) that are unknown to the node
    // TODO: What if the presignature completed generation and is already spent?
    pub async fn get_or_generate(
        &mut self,
        id: PresignatureId,
        triple0: TripleId,
        triple1: TripleId,
        triple_manager: &mut TripleManager,
        public_key: &PublicKey,
        private_share: &SecretKeyShare,
    ) -> Result<&mut PresignatureProtocol, GenerationError> {
        if self.presignatures.contains_key(&id) {
            Err(GenerationError::AlreadyGenerated)
        } else {
            match self.generators.entry(id) {
                Entry::Vacant(entry) => {
                    tracing::info!(id, "joining protocol to generate a new presignature");
                    let (triple0, triple1) =
                        match triple_manager.take_two(triple0, triple1, false).await {
                            Ok(result) => result,
                            Err(missing_triple_id) => {
                                tracing::warn!(
                                    triple0,
                                    triple1,
                                    "one of the triples is missing, can't join"
                                );
                                return Err(GenerationError::TripleIsMissing(missing_triple_id));
                            }
                        };
                    let generator = Self::generate_internal(
                        &self.participants,
                        self.me,
                        self.threshold,
                        triple0,
                        triple1,
                        public_key,
                        private_share,
                        false,
                    )?;
                    let generator = entry.insert(generator);
                    Ok(&mut generator.protocol)
                }
                Entry::Occupied(entry) => Ok(&mut entry.into_mut().protocol),
            }
        }
    }

    pub fn take_mine(&mut self) -> Option<Presignature> {
        tracing::info!(mine = ?self.mine, "my presignatures");
        let my_presignature_id = self.mine.pop_front()?;
        Some(self.presignatures.remove(&my_presignature_id).unwrap())
    }

    pub fn take(&mut self, id: PresignatureId) -> Option<Presignature> {
        self.presignatures.remove(&id)
    }

    /// Pokes all of the ongoing generation protocols and returns a vector of
    /// messages to be sent to the respective participant.
    ///
    /// An empty vector means we cannot progress until we receive a new message.
    pub fn poke(&mut self) -> Result<Vec<(Participant, PresignatureMessage)>, ProtocolError> {
        let mut messages = Vec::new();
        let mut result = Ok(());
        self.generators.retain(|id, generator| {
            loop {
                let action = match generator.poke() {
                    Ok(action) => action,
                    Err(e) => {
                        result = Err(e);
                        break false;
                    }
                };
                match action {
                    Action::Wait => {
                        tracing::debug!("waiting");
                        // Retain protocol until we are finished
                        return true;
                    }
                    Action::SendMany(data) => {
                        for p in &self.participants {
                            messages.push((
                                *p,
                                PresignatureMessage {
                                    id: *id,
                                    triple0: generator.triple0,
                                    triple1: generator.triple1,
                                    epoch: self.epoch,
                                    from: self.me,
                                    data: data.clone(),
                                },
                            ))
                        }
                    }
                    Action::SendPrivate(p, data) => messages.push((
                        p,
                        PresignatureMessage {
                            id: *id,
                            triple0: generator.triple0,
                            triple1: generator.triple1,
                            epoch: self.epoch,
                            from: self.me,
                            data: data.clone(),
                        },
                    )),
                    Action::Return(output) => {
                        tracing::info!(
                            id,
                            big_r = ?output.big_r.to_base58(),
                            "completed presignature generation"
                        );
                        self.presignatures
                            .insert(*id, Presignature { id: *id, output });
                        if generator.mine {
                            tracing::info!(id, "assigning presignature to myself");
                            self.mine.push_back(*id);
                        }
                        // Do not retain the protocol
                        return false;
                    }
                }
            }
        });
        result.map(|_| messages)
    }
}
