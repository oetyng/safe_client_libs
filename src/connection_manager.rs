// Copyright 2021 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::Error;
use bincode::serialize;
use futures::{
    future::{join_all, select_all},
    lock::Mutex,
};
use log::{debug, error, info, trace, warn};
use qp2p::{self, Config as QuicP2pConfig, Endpoint, IncomingMessages, QuicP2p};
use sn_data_types::{Keypair, PublicKey, Signature, TransferValidated};
use sn_messaging::{
    client::{Event, Message, QueryResponse},
    section_info::{
        Error as SectionInfoError, GetSectionResponse, Message as SectionInfoMsg, SectionInfo,
    },
    MessageId, MessageType, WireMsg,
};
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use threshold_crypto::PublicKeySet;
use tokio::{
    sync::mpsc::{channel, Sender, UnboundedSender},
    task::JoinHandle,
};
use xor_name::XorName;

static NUMBER_OF_RETRIES: usize = 3;
pub static STANDARD_ELDERS_COUNT: usize = 5;

/// Simple map for correlating a response with votes from various elder responses.
type VoteMap = HashMap<[u8; 32], (QueryResponse, usize)>;

// channel for sending result of transfer validation
type TransferValidationSender = Sender<Result<TransferValidated, Error>>;
type QueryResponseSender = Sender<Result<QueryResponse, Error>>;

//type ElderConnectionMap = HashSet<SocketAddr>;

type PendingTransferValidations = Arc<Mutex<HashMap<MessageId, TransferValidationSender>>>;
type PendingQueryResponses = Arc<Mutex<HashMap<(SocketAddr, MessageId), QueryResponseSender>>>;

/// Initialises `QuicP2p` instance which can bootstrap to the network, establish
/// connections and send messages to several nodes, as well as await responses from them.
pub struct ConnectionManager {}

impl ConnectionManager {
    /// Bootstrap to the network maintaining connections to several nodes.
    pub async fn bootstrap(session: Session) -> Result<Session, Error> {
        // &'static
        trace!(
            "Trying to bootstrap to the network with public_key: {:?}",
            session.client_public_key()
        );

        // Bootstrap and send a handshake request to
        let (session, mut incoming_messages) = Self::get_section(session).await?;
        // Receive the list of Elders we can then connect to
        let session = Self::get_elders(session, &mut incoming_messages).await?;
        // Let's now connect to all Elders
        let session = Self::connect_to_elders(session).await?;

        session.listen_to_incoming_messages(incoming_messages).await
    }

    async fn get_elders(
        session: Session,
        incoming_messages: &mut IncomingMessages,
    ) -> Result<Session, Error> {
        trace!("getting elders..");
        if let Some((_src, message)) = incoming_messages.next().await {
            trace!("got response when getting elders");
            match SectionInfoMsg::from(message) {
                Ok(msg) => ConnectionManager::handle_sectioninfo_msg(msg, session).await,
                Err(e) => Err(e.into()),
            }
        } else {
            trace!("incoming_messages.next().await was None");
            Err(Error::NotBootstrapped)
        }
    }

    /// Send a `Message` to the network without awaiting for a response.
    pub async fn send_cmd(
        msg: &Message,
        endpoint: Endpoint,
        elders: Vec<SocketAddr>,
    ) -> Result<(), Error> {
        let msg_id = msg.id();

        let endpoint = endpoint.clone();
        let src_addr = endpoint.socket_addr();
        trace!(
            "Sending (from {}) command message {:?} w/ id: {:?}",
            src_addr,
            msg,
            msg_id
        );
        let msg_bytes = msg.serialize()?;

        // Send message to all Elders concurrently
        let mut tasks = Vec::default();

        //let elders_addrs: Vec<SocketAddr> = elders.iter().cloned().collect();
        // clone elders as we want to update them in this process
        for socket in elders {
            let msg_bytes_clone = msg_bytes.clone();
            let endpoint = endpoint.clone();
            let task_handle: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
                trace!("About to send cmd message {:?} to {:?}", msg_id, &socket);
                endpoint.connect_to(&socket).await?;
                endpoint.send_message(msg_bytes_clone, &socket).await?;

                trace!("Sent cmd with MsgId {:?}to {:?}", msg_id, &socket);
                Ok(())
            });
            tasks.push(task_handle);
        }

        // Let's await for all messages to be sent
        let results = join_all(tasks).await;

        let mut failures = 0;
        results.iter().for_each(|res| {
            if res.is_err() {
                failures += 1;
            }
        });

        if failures > 0 {
            error!("Sending the message to {} Elders failed", failures);
        }

        Ok(())
    }

    /// Remove a pending transfer sender from the listener map
    pub async fn remove_pending_transfer_sender(
        msg_id: &MessageId,
        pending_transfers: PendingTransferValidations,
    ) -> Result<(), Error> {
        trace!("Removing pending transfer sender");
        let mut listeners = pending_transfers.lock().await;
        let _ = listeners
            .remove(msg_id)
            .ok_or(Error::NoTransferValidationListener)?;
        Ok(())
    }

    /// Send a transfer validation message to all Elder without awaiting for a response.
    pub async fn send_transfer_validation(
        msg: &Message,
        sender: Sender<Result<TransferValidated, Error>>,
        endpoint: Endpoint,
        elders: Vec<SocketAddr>,
        pending_transfers: PendingTransferValidations,
    ) -> Result<(), Error> {
        info!(
            "Sending transfer validation command {:?} w/ id: {:?}",
            msg,
            msg.id()
        );
        let msg_bytes = msg.serialize()?;

        let msg_id = msg.id();
        let _ = pending_transfers.lock().await.insert(msg_id, sender);

        // Send message to all Elders concurrently
        let mut tasks = Vec::default();
        for socket in elders.iter() {
            let msg_bytes_clone = msg_bytes.clone();
            let socket = *socket;

            let endpoint = endpoint.clone();

            let task_handle = tokio::spawn(async move {
                endpoint.connect_to(&socket).await?;
                trace!("Sending transfer validation to Elder {}", &socket);
                endpoint.send_message(msg_bytes_clone, &socket).await?;
                Ok::<_, Error>(())
            });
            tasks.push(task_handle);
        }

        // Let's await for all messages to be sent
        let _results = join_all(tasks).await;

        // TODO: return an error if we didn't successfully
        // send it to at least a majority of Elders??

        Ok(())
    }

    /// Send a Query `Message` to the network awaiting for the response.
    pub async fn send_query(
        msg: &Message,
        endpoint: Endpoint,
        elders: Vec<SocketAddr>,
        pending_queries: PendingQueryResponses,
    ) -> Result<QueryResponse, Error> {
        info!("sending query message {:?} w/ id: {:?}", msg, msg.id());
        let msg_bytes = msg.serialize()?;

        // We send the same message to all Elders concurrently,
        // and we try to find a majority on the responses
        let mut tasks = Vec::default();

        for socket in elders.clone() {
            // Create a new stream here to not have to worry about filtering replies
            let msg_id = msg.id();
            let msg = msg.clone();
            let msg_bytes_clone = msg_bytes.clone();
            let pending_queries = pending_queries.clone();
            let endpoint = endpoint.clone();
            endpoint.connect_to(&socket).await?;

            let task_handle = tokio::spawn(async move {
                // Retry queries that failed for connection issues
                let mut done_trying = false;
                let mut result = Err(Error::ElderQuery);
                let mut attempts: usize = 1;

                while !done_trying {
                    let msg_bytes_clone = msg_bytes_clone.clone();

                    let (sender, mut receiver) = channel::<Result<QueryResponse, Error>>(7);
                    let _ = pending_queries
                        .lock()
                        .await
                        .insert((socket, msg_id), sender);

                    // TODO: we need to remove the msg_id from
                    // pending_queries upon any failure below
                    match endpoint.send_message(msg_bytes_clone, &socket).await {
                        Ok(()) => {
                            trace!(
                                "Message {:?} sent to {}. Waiting for response...",
                                msg.clone(),
                                &socket
                            );

                            if let Some(res) = receiver.recv().await {
                                return Ok(res?);
                            } else {
                                error!("Error from query response, non received");
                                return Err(Error::QueryReceiverError);
                            }
                        }
                        Err(_error) => {
                            result = {
                                error!("Error sending query message");
                                // TODO: remove it from the pending_query_responses then
                                Err(Error::SendingQuery)
                            }
                        }
                    };

                    debug!(
                        "Try #{:?} @ {:?}. Got back response: {:?}",
                        attempts,
                        socket,
                        &result.is_ok()
                    );

                    if result.is_ok() || attempts > NUMBER_OF_RETRIES {
                        done_trying = true;
                    }

                    attempts += 1;
                }

                result
            });

            tasks.push(task_handle);
        }

        // Let's figure out what's the value which is in the majority of responses obtained
        let mut vote_map = VoteMap::default();
        let mut received_errors = 0;

        // 2/3 of known elders
        let threshold: usize = (elders.len() as f32 / 2_f32).ceil() as usize;

        trace!("Vote threshold is: {:?}", threshold);
        let mut winner: (Option<QueryResponse>, usize) = (None, threshold);

        // Let's await for all responses
        let mut has_elected_a_response = false;
        let mut todo = tasks;

        while !has_elected_a_response {
            if todo.is_empty() {
                warn!("No more connections to try");
                break;
            }

            let (res, _idx, remaining_futures) = select_all(todo.into_iter()).await;
            todo = remaining_futures;
            if let Ok(res) = res {
                match res {
                    Ok(response) => {
                        debug!("QueryResponse received is: {:#?}", response);

                        // bincode here as we're using the internal qr, without serialisation
                        // this is only used internally to sn_client
                        let key = tiny_keccak::sha3_256(&serialize(&response)?);
                        let (_, counter) = vote_map.entry(key).or_insert((response.clone(), 0));
                        *counter += 1;

                        // First, see if this latest response brings us above the threshold for any response
                        if *counter > threshold {
                            trace!("Enough votes to be above response threshold");

                            winner = (Some(response.clone()), *counter);
                            has_elected_a_response = true;
                        }
                    }
                    _ => {
                        warn!("Unexpected message in reply to query (retrying): {:?}", res);
                        received_errors += 1;
                    }
                }
            } else if let Err(error) = res {
                error!("Error spawning query task: {:?} ", error);
                received_errors += 1;
            }

            // Second, let's handle no winner if we have > threshold responses.
            if !has_elected_a_response {
                winner = Self::select_best_of_the_rest_response(
                    winner,
                    threshold,
                    &vote_map,
                    received_errors,
                    &mut has_elected_a_response,
                );
                has_elected_a_response = true;
                trace!("LOOOOG: 5");
            } else {
                trace!("LOOOOG: 6");
            }
        }

        debug!(
            "Response obtained after querying {} nodes: {:?}",
            winner.1, winner.0
        );

        winner.0.ok_or(Error::NoResponse)
    }

    /// Choose the best response when no single responses passes the threshold
    fn select_best_of_the_rest_response(
        current_winner: (Option<QueryResponse>, usize),
        threshold: usize,
        vote_map: &VoteMap,
        received_errors: usize,
        has_elected_a_response: &mut bool,
    ) -> (Option<QueryResponse>, usize) {
        trace!("No response selected yet, checking if fallback needed");
        let mut number_of_responses = 0;
        let mut most_popular_response = current_winner;

        for (_, (message, votes)) in vote_map.iter() {
            number_of_responses += votes;
            trace!(
                "Number of votes cast :{:?}. Threshold is: {:?} votes",
                number_of_responses,
                threshold
            );

            number_of_responses += received_errors;

            trace!(
                "Total number of responses (votes and errors) :{:?}",
                number_of_responses
            );

            if most_popular_response.0 == None {
                trace!("LOOOOG: 0");
                most_popular_response = (Some(message.clone()), *votes);
            }

            if votes > &most_popular_response.1 {
                trace!("Reselecting winner, with {:?} votes: {:?}", votes, message);

                most_popular_response = (Some(message.clone()), *votes)
            } else {
                trace!("LOOOOG: 1");
                // TODO: check w/ farming we get a proper history returned w /matching responses.
                if let QueryResponse::GetHistory(Ok(history)) = &message {
                    trace!("LOOOOG: 2");
                    // if we're not more popular but in simu payout mode, check if we have more history...
                    if cfg!(feature = "simulated-payouts") && votes == &most_popular_response.1 {
                        trace!("LOOOOG: 3");
                        if let Some(QueryResponse::GetHistory(Ok(popular_history))) =
                            &most_popular_response.0
                        {
                            trace!("LOOOOG: 4");
                            if history.len() > popular_history.len() {
                                trace!("GetHistory response received in Simulated Payouts... choosing longest history. {:?}", history);
                                most_popular_response = (Some(message.clone()), *votes)
                            }
                        }
                    }
                }
            }
        }

        if number_of_responses > threshold {
            trace!("No clear response above the threshold, so choosing most popular response with: {:?} votes: {:?}", most_popular_response.1, most_popular_response.0);
            *has_elected_a_response = true;
        }

        most_popular_response
    }

    // Private helpers

    // Bootstrap to the network to obtaining the list of
    // nodes we should establish connections with
    async fn get_section(mut session: Session) -> Result<(Session, IncomingMessages), Error> {
        trace!("Bootstrapping with contacts...");
        let (
            endpoint,
            _incoming_connections,
            incoming_messages,
            _disconnections,
            bootstrapped_peer,
        ) = session.qp2p.bootstrap().await?;
        //self.endpoint = Some(endpoint);

        // 1. We query the network for section info.
        trace!("Querying for section info from bootstrapped node...");
        let msg = SectionInfoMsg::GetSectionQuery(XorName::from(session.client_public_key()))
            .serialize()?;

        session.endpoint = Some(endpoint);

        session
            .endpoint()?
            .send_message(msg, &bootstrapped_peer)
            .await?;

        Ok((session, incoming_messages))
    }

    // Connect to a set of Elders nodes which will be
    // the receipients of our messages on the network.
    async fn connect_to_elders(mut session: Session) -> Result<Session, Error> {
        // Connect to all Elders concurrently
        // We spawn a task per each node to connect to
        let mut tasks = Vec::default();

        let endpoint = session.endpoint()?;
        let msg = session.bootstrap_cmd().await?;
        let peers = session.elders.clone();
        debug!(
            "Sending bootstrap cmd from {} to {} peers..",
            endpoint.socket_addr(),
            peers.len()
        );

        for peer_addr in peers {
            let endpoint = endpoint.clone();
            let msg = msg.clone();
            let task_handle = tokio::spawn(async move {
                let mut result = Err(Error::ElderConnection);
                let mut connected = false;
                let mut attempts: usize = 0;
                while !connected && attempts <= NUMBER_OF_RETRIES {
                    attempts += 1;
                    endpoint.connect_to(&peer_addr).await?;
                    endpoint.send_message(msg.clone(), &peer_addr).await?;
                    connected = true;

                    debug!(
                        "Elder conn attempt #{} @ {} is connected? : {:?}",
                        attempts, peer_addr, connected
                    );

                    result = Ok(peer_addr)
                }

                result
            });
            tasks.push(task_handle);
        }

        // TODO: Do we need a timeout here to check sufficient time has passed + or sufficient connections?
        let mut has_sufficent_connections = false;
        let mut todo = tasks;
        let mut elders = HashSet::new();

        while !has_sufficent_connections {
            if todo.is_empty() {
                warn!("No more elder connections to try");
                break;
            }

            let (res, _idx, remaining_futures) = select_all(todo.into_iter()).await;
            if remaining_futures.is_empty() {
                has_sufficent_connections = true;
            }

            todo = remaining_futures;

            if let Ok(elder_result) = res {
                let res = elder_result.map_err(|err| {
                    // elder connection retires already occur above
                    warn!("Failed to connect to Elder @ : {}", err);
                });

                if let Ok(socket_addr) = res {
                    info!("Connected to elder: {:?}", socket_addr);
                    let _ = elders.insert(socket_addr);
                }
            }

            // TODO: this will effectively stop driving futures after we get 2...
            // We should still let all progress... just without blocking
            if elders.len() >= STANDARD_ELDERS_COUNT {
                has_sufficent_connections = true;
            }
            if elders.len() < STANDARD_ELDERS_COUNT {
                warn!("Connected to only {:?} elders.", elders.len());
            }
            if elders.len() < STANDARD_ELDERS_COUNT - 2 && has_sufficent_connections {
                return Err(Error::InsufficientElderConnections);
            }
        }

        trace!("Connected to {} Elders.", elders.len());

        session.elders = elders;

        Ok(session)
    }

    /// Handle received network info messages
    async fn handle_sectioninfo_msg(
        msg: SectionInfoMsg,
        mut session: Session,
    ) -> Result<Session, Error> {
        debug!("Handling network info message");
        match &msg {
            SectionInfoMsg::GetSectionResponse(GetSectionResponse::Success(info)) => {
                trace!("GetSectionResponse Success! Elders: ({:?})", info.elders);
                ConnectionManager::update_session_info(session, info).await
            }
            SectionInfoMsg::RegisterEndUserError(error)
            | SectionInfoMsg::GetSectionResponse(GetSectionResponse::SectionInfoUpdate(error)) => {
                error!("Message {:?} was interrupted due to {:?}. This will most likely need to be sent again.", msg, error);
                if let SectionInfoError::TargetSectionInfoOutdated(info) = error {
                    trace!("Updated network info: ({:?})", info);
                    session = ConnectionManager::update_session_info(session, info).await?;
                }
                Ok(session)
            }
            SectionInfoMsg::GetSectionResponse(GetSectionResponse::Redirect(addresses)) => {
                trace!("GetSectionResponse::Redirect, trying with provided elders");
                session.elders = addresses.iter().copied().collect();
                // Continually try and bootstrap against new elders while we're getting rediret
                let (session, _) = Self::get_section(session).await?;
                Ok(session)
            }
            SectionInfoMsg::SectionInfoUpdate(update) => {
                let correlation_id = update.correlation_id;
                error!("MessageId {:?} was interrupted due to infrastructure updates. This will most likely need to be sent again. Update was : {:?}", correlation_id, update);
                if let SectionInfoError::TargetSectionInfoOutdated(info) = update.clone().error {
                    trace!("Updated network info: ({:?})", info);
                    session = ConnectionManager::update_session_info(session, &info).await?;
                }
                Ok(session)
            }
            SectionInfoMsg::RegisterEndUserCmd { .. } | SectionInfoMsg::GetSectionQuery(_) => {
                Err(Error::UnexpectedMessageOnJoin(format!(
                    "bootstrapping failed since an invalid response ({:?}) was received",
                    msg
                )))
            }
        }
    }

    /// Apply updated info to a network session, and trigger connections
    async fn update_session_info(
        mut session: Session,
        info: &SectionInfo,
    ) -> Result<Session, Error> {
        let elders = &info.elders;
        session.section_key_set = Some(info.pk_set.clone());
        trace!("GetSectionResponse Success! Elders: ({:?})", elders);
        // Obtain the addresses of the Elders
        let elders_addrs = elders
            .iter()
            .map(|(_, socket_addr)| socket_addr)
            .copied()
            .collect();
        // clear existing elder list.
        session.elders = elders_addrs;
        Self::connect_to_elders(session).await
    }

    /// Handle messages intended for client consumption (re: queries + commands)
    async fn handle_client_msg(msg: Message, src: SocketAddr, session: Session) -> Session {
        let notifier = session.notifier.clone();
        match msg.clone() {
            Message::QueryResponse {
                response,
                correlation_id,
                ..
            } => {
                trace!("Query response in: {:?}", response);

                if let Some(mut sender) = session
                    .pending_queries
                    .lock()
                    .await
                    .remove(&(src, correlation_id))
                {
                    trace!("Sender channel found for query response");
                    let _ = sender.send(Ok(response)).await;
                } else {
                    warn!(
                        "No matching pending query found for elder {:?}  and message {:?}",
                        src, correlation_id
                    );
                }
            }
            Message::Event {
                event,
                correlation_id,
                ..
            } => {
                if let Event::TransferValidated { event, .. } = event {
                    if let Some(sender) = session
                        .pending_transfers
                        .lock()
                        .await
                        .get_mut(&correlation_id)
                    {
                        info!("Accumulating SignatureShare");
                        let _ = sender.send(Ok(event)).await;
                    } else {
                        warn!("No matching transfer validation event listener found for elder {:?} and message {:?}", src, correlation_id);
                        warn!("It may be that this transfer is complete and the listener cleaned up already.");
                        trace!("Event received was {:?}", event);
                    }
                }
            }
            Message::CmdError {
                error,
                correlation_id,
                ..
            } => {
                if let Some(sender) = session
                    .pending_transfers
                    .lock()
                    .await
                    .get_mut(&correlation_id)
                {
                    debug!("Cmd Error was received, sending on channel to caller");
                    let _ = sender.send(Err(Error::from(error.clone()))).await;
                } else {
                    warn!("No sender subscribing and listening for errors relating to message {:?}. Error returned is: {:?}", correlation_id, error)
                }

                let _ = notifier.send(Error::from(error));
            }
            msg => {
                warn!("another message type received {:?}", msg);
            }
        };
        session
    }
}

#[derive(Clone)]
pub struct Session {
    pub qp2p: QuicP2p,
    pub notifier: UnboundedSender<Error>,
    pub pending_queries: PendingQueryResponses,
    pub pending_transfers: PendingTransferValidations,
    pub endpoint: Option<Endpoint>,
    pub elders: HashSet<SocketAddr>,
    pub section_key_set: Option<PublicKeySet>,
    pub signer: Signer,
}

impl Session {
    pub fn new(
        qp2p_config: QuicP2pConfig,
        signer: Signer,
        notifier: UnboundedSender<Error>,
    ) -> Result<Self, Error> {
        debug!("QP2p config: {:?}", qp2p_config);

        let qp2p = qp2p::QuicP2p::with_config(Some(qp2p_config), Default::default(), false)?;
        Ok(Session {
            qp2p,
            notifier,
            pending_queries: Arc::new(Mutex::new(HashMap::default())),
            pending_transfers: Arc::new(Mutex::new(HashMap::default())),
            endpoint: None,
            section_key_set: None,
            elders: HashSet::default(),
            signer,
        })
    }

    pub fn client_public_key(&self) -> PublicKey {
        self.signer.public_key()
    }

    pub fn endpoint(&self) -> Result<&Endpoint, Error> {
        match self.endpoint.borrow() {
            Some(endpoint) => Ok(endpoint),
            None => {
                trace!("self.endpoint.borrow() was None");
                Err(Error::NotBootstrapped)
            }
        }
    }

    pub fn section_key(&self) -> Result<PublicKey, Error> {
        match self.section_key_set.borrow() {
            Some(section_key_set) => Ok(PublicKey::Bls(section_key_set.public_key())),
            None => {
                trace!("self.section_key_set.borrow() was None");
                Err(Error::NotBootstrapped)
            }
        }
    }

    pub async fn section_key_set(&self) -> Result<&PublicKeySet, Error> {
        match self.section_key_set.borrow() {
            Some(section_key_set) => Ok(section_key_set),
            None => {
                trace!("self.section_key_set.borrow() was None");
                Err(Error::NotBootstrapped)
            }
        }
    }

    pub async fn bootstrap_cmd(&self) -> Result<bytes::Bytes, Error> {
        let socketaddr_sig = self
            .signer
            .sign(&serialize(&self.endpoint()?.socket_addr())?)
            .await?;
        SectionInfoMsg::RegisterEndUserCmd {
            end_user: self.client_public_key(),
            socketaddr_sig,
        }
        .serialize()
        .map_err(Error::MessagingProtocol)
    }

    /// Listen for incoming messages on a connection
    pub async fn listen_to_incoming_messages(
        self,
        mut incoming_messages: IncomingMessages,
    ) -> Result<Session, Error> {
        // &'static
        debug!("Adding IncomingMessages listener");

        let mut session = self.clone();
        // Spawn a thread
        let _ = tokio::spawn(async move {
            while let Some((src, message)) = incoming_messages.next().await {
                let message_type = WireMsg::deserialize(message)?;
                warn!("Message received at listener from {:?}", &src);
                let session_clone = session.clone();
                session = match message_type {
                    MessageType::SectionInfo(msg) => {
                        match ConnectionManager::handle_sectioninfo_msg(msg, session).await {
                            Ok(session) => session,
                            Err(error) => {
                                error!("Error handling network info message: {:?}", error);
                                // that's enough
                                // go back to using a clone of session before the error
                                session_clone
                            }
                        }
                    }
                    MessageType::ClientMessage(msg) => {
                        ConnectionManager::handle_client_msg(msg, src, session).await
                    }
                    msg_type => {
                        warn!("Unexpected message type received: {:?}", msg_type);
                        session
                    }
                };
            }
            info!("IncomingMessages listener is closing now");
            Ok::<(), Error>(())
        });

        // Some or None, not super important if this existed before...
        Ok(self)
    }
}

#[derive(Clone)]
pub struct Signer {
    keypair: Arc<Mutex<Keypair>>,
    public_key: PublicKey,
}

impl Signer {
    pub fn new(keypair: Keypair) -> Self {
        let public_key = keypair.public_key();
        Self {
            keypair: Arc::new(Mutex::new(keypair)),
            public_key,
        }
    }

    pub fn public_key(&self) -> PublicKey {
        self.public_key
    }

    pub async fn sign(&self, data: &[u8]) -> Result<Signature, Error> {
        Ok(self.keypair.lock().await.sign(data))
    }
}
