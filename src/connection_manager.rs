// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::config_handler::Config;
use crate::Error;
use bincode::serialize;
use bytes::Bytes;
use futures::{
    future::{join_all, select_all},
    lock::Mutex,
};
use log::{debug, error, info, trace, warn};
use qp2p::{self, Config as QuicP2pConfig, Endpoint, IncomingMessages, QuicP2p};
use sn_data_types::{HandshakeRequest, Keypair, ReplicaPublicKeySet, TransferValidated};
use sn_messaging::{
    client::{Event, Message, QueryResponse},
    network_info::{GetSectionResponse, Message as NetworkInfoMsg, NetworkInfo, Error as InfrastructureUpdate},
    MessageId,
    WireMsg,
    MessageType
};

use futures::StreamExt;
use std::iter::Iterator;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::mpsc::{channel, Sender, UnboundedSender, Receiver},
    task::JoinHandle,
    time::timeout,
};
use xor_name::XorName;

static NUMBER_OF_RETRIES: usize = 3;
static RESPONSE_WAIT_TIME: u64 = 30;
pub static STANDARD_ELDERS_COUNT: usize = 5;

/// Simple map for correlating a response with votes from various elder responses.
type VoteMap = HashMap<[u8; 32], (QueryResponse, usize)>;

// channel for sending result of transfer validation
type TransferValidationSender = Sender<Result<TransferValidated, Error>>;
type QueryResponseSender = Sender<Result<QueryResponse, Error>>;

type ElderConnectionMap = HashSet<SocketAddr>;
type KeySetSender = Sender<Result<ReplicaPublicKeySet, Error>>;
/// Initialises `QuicP2p` instance which can bootstrap to the network, establish
/// connections and send messages to several nodes, as well as await responses from them.
#[derive(Clone)]
pub struct ConnectionManager {
    keypair: Keypair,
    qp2p: QuicP2p,
    elders: Arc<Mutex<ElderConnectionMap>>,
    endpoint: Arc<Mutex<Option<Endpoint>>>,
    pending_transfer_validations: Arc<Mutex<HashMap<MessageId, TransferValidationSender>>>,
    pending_query_responses: Arc<Mutex<HashMap<(SocketAddr, MessageId), QueryResponseSender>>>,
    notification_sender: Arc<Mutex<UnboundedSender<Error>>>,
    
    // receive the pk set when calling bootstrap func
    keyset_sender: Arc<Mutex<Sender<Result<ReplicaPublicKeySet, Error>>>>,
    keyset_receiver: Arc<Mutex<Receiver<Result<ReplicaPublicKeySet, Error>>>>,
    // network_listener: Arc<Mutex<JoinHandle<Result<(), Error>>>>
    // keyset_channel: Arc<Mutex<Option<KeySetSender>>>
    // keyset_channel: <Arc<Mutex<Option<Sender<Result<ReplicaPublicKeySet, Error>>>>>,
    // config_file_path: Option<&'static Path>,
}

impl ConnectionManager {
    /// Create a new connection manager.
    pub async fn new(
        config: QuicP2pConfig,
        // config_file_path: Option<&'static Path>,
        // bootstrap_config: Option<HashSet<SocketAddr>>,
        keypair: Keypair,
        notification_sender: UnboundedSender<Error>,
    ) -> Result<Self, Error> {
        // let mut config = Config::new(config_file_path, bootstrap_config).qp2p;

        let qp2p = QuicP2p::with_config(Some(config), Default::default(), false)?;
        let (sender, receiver) = channel::<Result<ReplicaPublicKeySet, Error>>(1);

        Ok(Self {
            keypair,
            qp2p,
            elders: Arc::new(Mutex::new(HashSet::default())),
            endpoint: Arc::new(Mutex::new(None)),
            pending_transfer_validations: Arc::new(Mutex::new(HashMap::default())),
            pending_query_responses: Arc::new(Mutex::new(HashMap::default())),
            notification_sender: Arc::new(Mutex::new(notification_sender)),
            // config_file_path,
            keyset_sender:Arc::new(Mutex::new(sender)),
            keyset_receiver:Arc::new(Mutex::new(receiver)),
        })
    }

    /// Loop bootstrap attempts up to three times
    pub async fn retry_bootstrap(self, bootstrap_config: &Vec<SocketAddr> ) -> Result<(Self, ReplicaPublicKeySet), Error> {
        let mut attempts: u32 = 0;

        loop {
            trace!("bootstrap attempt, {:?}", attempts);
            let res = self.clone().bootstrap(bootstrap_config).await;
            match res {
                Ok(pk_set) => {
                    // debug!(">>>>> bootstra done... endpoint is some?{:?}", self.endpoint.is_some());
                    return Ok((self, pk_set))
                },
                Err(err) => {
                    attempts += 1;
                    if attempts < 3 {
                        trace!("Error connecting to network! Retrying... ({})", attempts);
                    } else {
                        return Err(err);
                    }
                }
            }
        }

    }

    /// Bootstrap to the network maintaining connections to several nodes.
    pub async fn bootstrap(
        &self,
        // qp2p_config: QuicP2pConfig,
        bootstrap_config: &Vec<SocketAddr>,
    ) -> Result<ReplicaPublicKeySet, Error> {
        trace!(
            "Trying to bootstrap to the network with public_key: {:?}",
            self.keypair.public_key()
        );

        // Bootstrap and send a handshake request to receive
        // the list of Elders we can then connect to
        let incoming_messages = self.get_section(bootstrap_config).await?;
        
        
        let mut receiver = self.keyset_receiver.lock().await;
        // let (_, receiver) = channel;
        // keyset_sender = Some(sender);

            {

                debug!("11111 endpoint is: {:?}", self.endpoint.lock().await.is_some());
            }
        let handle = self.listen_to_incoming_messages(incoming_messages).await;

        // debug!("2222 endpoint is: {:?}", self.endpoint.lock().await.is_some());
        {

            debug!("22222 endpoint is: {:?}", self.endpoint.lock().await.is_some());
        }

        debug!("waitinggggggggggg");
        // wait on our section PK set to be received before progressing
        if let Some(res) = receiver.next().await {
            let pk_set = res?;

            Ok(pk_set)
        } else {
            debug!("--->>>> in boot");
            Err(Error::NotBootstrapped)
        }
    }

    /// Send a `Message` to the network without awaiting for a response.
    pub async fn send_cmd(&mut self, msg: &Message) -> Result<(), Error> {
        let msg_id = msg.id();
        debug!("--->>>> in send_cmd");

        let endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;
        let src_addr = endpoint.socket_addr();
        info!(
            "Sending (from {}) command message {:?} w/ id: {:?}",
            src_addr, msg, msg_id
        );
        let msg_bytes = msg.serialize()?;

        // Send message to all Elders concurrently
        let mut tasks = Vec::default();

        let elders = self.elders.lock().await;
        let elders_addrs: Vec<SocketAddr> = elders.iter().cloned().collect();
        // clone elders as we want to update them in this process
        for socket in elders_addrs {
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
    pub async fn remove_pending_transfer_sender(&self, msg_id: &MessageId) -> Result<(), Error> {
        trace!("Removing pending transfer sender");
        let mut listeners = self.pending_transfer_validations.lock().await;

        let _ = listeners
            .remove(msg_id)
            .ok_or(Error::NoTransferValidationListener)?;

        Ok(())
    }

    /// Send a transfer validation message to all Elder without awaiting for a response.
    pub async fn send_transfer_validation(
        &self,
        msg: &Message,
        sender: Sender<Result<TransferValidated, Error>>,
    ) -> Result<(), Error> {
        info!(
            "Sending transfer validation command {:?} w/ id: {:?}",
            msg,
            msg.id()
        );
        let msg_bytes = msg.serialize()?;

        let msg_id = msg.id();
        {
            let _ = self
                .pending_transfer_validations
                .lock()
                .await
                .insert(msg_id, sender);
        }

        // Send message to all Elders concurrently
        let mut tasks = Vec::default();
        let elders = self.elders.lock().await;

        for socket in elders.iter() {
            let msg_bytes_clone = msg_bytes.clone();
            let socket = *socket;

            let endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;

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
    pub async fn send_query(&mut self, msg: &Message) -> Result<QueryResponse, Error> {
        info!("sending query message {:?} w/ id: {:?}", msg, msg.id());
        let msg_bytes = msg.serialize()?;

        // We send the same message to all Elders concurrently,
        // and we try to find a majority on the responses
        let mut tasks = Vec::default();
        let elders = self.elders.lock().await;
        // let endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;
        let elders_addrs: Vec<SocketAddr> = elders.iter().cloned().collect();
        for socket in elders_addrs {
            let msg_bytes_clone = msg_bytes.clone();
            // Create a new stream here to not have to worry about filtering replies
            let msg_id = msg.id();

            let pending_query_responses = self.pending_query_responses.clone();

            let mut endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;
            endpoint.connect_to(&socket).await?;

            let task_handle = tokio::spawn(async move {
                // Retry queries that failed for connection issues
                let mut done_trying = false;
                let mut result = Err(Error::ElderQuery);
                let mut attempts: usize = 1;

                while !done_trying {
                    let msg_bytes_clone = msg_bytes_clone.clone();

                    let (sender, mut receiver) = channel::<Result<QueryResponse, Error>>(7);
                    {
                        let _ = pending_query_responses
                            .lock()
                            .await
                            .insert((socket, msg_id), sender);
                    }

                    // TODO: we need to remove the msg_id from
                    // pending_query_responses upon any failure below
                    match endpoint.send_message(msg_bytes_clone, &socket).await {
                        Ok(()) => {
                            trace!("Message sent to {}. Waiting for response...", &socket);
                            // TODO: receive response here.
                            result = match timeout(
                                Duration::from_secs(RESPONSE_WAIT_TIME),
                                receiver.recv(),
                            )
                            .await
                            {
                                Ok(Some(result)) => match result {
                                    Ok(response) => Ok(response),
                                    Err(_) => Err(Error::ReceivingQuery),
                                },
                                Ok(None) => Err(Error::ReceivingQuery),
                                Err(err) => {
                                    warn!("{}", err);
                                    // Timeout while waiting for response.
                                    // Terminate all connections to the peer
                                    endpoint.disconnect_from(&socket)?;
                                    Err(Error::ReceivingQuery)
                                }
                            };
                        }
                        Err(_error) => {
                            result = {
                                // TODO: remove it from the pending_query_responses then
                                Err(Error::ReceivingQuery)
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
        let elders = self.elders.lock().await;

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
                winner = self.select_best_of_the_rest_response(
                    winner,
                    threshold,
                    &vote_map,
                    received_errors,
                    &mut has_elected_a_response,
                );
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
        &self,
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
                most_popular_response = (Some(message.clone()), *votes);
            }

            if votes > &most_popular_response.1 {
                trace!("Reselecting winner, with {:?} votes: {:?}", votes, message);

                most_popular_response = (Some(message.clone()), *votes)
            } else {
                // TODO: check w/ farming we get a proper history returned w /matching responses.
                if let QueryResponse::GetHistory(Ok(history)) = &message {
                    // if we're not more popular but in simu payout mode, check if we have more history...
                    if cfg!(feature = "simulated-payouts") && votes == &most_popular_response.1 {
                        if let Some(QueryResponse::GetHistory(Ok(popular_history))) =
                            &most_popular_response.0
                        {
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
    async fn get_section(&self, bootstrap_nodes_override: &Vec<SocketAddr>) -> Result<IncomingMessages, Error> {
        info!("Sending NetworkInfo::GetSectionRequest");

        trace!("override nodes: {:?}", bootstrap_nodes_override);

        // let qp2p = QuicP2p::with_config(Some(qp2p_config), Default::default(), false)?;
        // overwrite our qp2p instance with out new bootstrapped one
        // self.qp2p = qp2p;

        let (
            the_endpoint,
            _incoming_connections,
            incoming_messages,
            _disconnections,
            bootstrapped_peer,
        ) = self.qp2p.bootstrap(bootstrap_nodes_override).await?;
        
        {
            let mut endpoint = self.endpoint.lock().await;
            *endpoint = Some(the_endpoint);
        }
 

        trace!("Sending handshake request to bootstrapped node...");
        let public_key = self.keypair.public_key();
        let xorname = XorName::from(public_key);
        let msg = NetworkInfoMsg::GetSectionQuery(xorname).serialize()?;

        let endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;
        endpoint.send_message(msg, &bootstrapped_peer).await?;
        trace!("get section done");
        Ok(incoming_messages)
    }

    pub async fn number_of_connected_elders(&self) -> usize {
        let elders = self.elders.lock().await;

        elders.len()
    }

    // Connect to a set of Elders nodes which will be
    // the receipients of our messages on the network.
    async fn connect_to_elders(&self, elders_addrs: Vec<SocketAddr>) -> Result<(), Error> {
        // Connect to all Elders concurrently
        // We spawn a task per each node to connect to
        let mut tasks = Vec::default();

        let endpoint = self.endpoint.lock().await.clone().ok_or(Error::NotBootstrapped)?;
        for peer_addr in elders_addrs {
            let keypair = self.keypair.clone();

            let endpoint = endpoint.clone();
            let task_handle = tokio::spawn(async move {
                let mut result = Err(Error::ElderConnection);
                let mut connected = false;
                let mut attempts: usize = 0;
                while !connected && attempts <= NUMBER_OF_RETRIES {
                    let public_key = keypair.public_key();
                    attempts += 1;
                    endpoint.connect_to(&peer_addr).await?;

                    let handshake = HandshakeRequest::Join(public_key);
                    let msg = Bytes::from(serialize(&handshake)?);

                    endpoint.send_message(msg, &peer_addr).await?;

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

        trace!("Connection threads have been setup.");

        // TODO: Do we need a timeout here to check sufficient time has passed + or sufficient connections?
        let mut has_sufficent_connections = false;

        let mut todo = tasks;

        let mut elders = self.elders.lock().await;
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
        Ok(())
    }

    /// Listen for incoming messages on a connection
    pub async fn listen_to_incoming_messages(
        &self,
        mut incoming_messages: IncomingMessages,
    ) -> JoinHandle<Result<(), Error>> {
        debug!("Adding IncomingMessages listener");
        
        let cm = self.clone();
        // Spawn a thread for listening
        tokio::spawn(async move {
            trace!("Listener thread spawned");

            while let Some((src, message)) = incoming_messages.next().await {
                debug!("MESSAGE {:?}", message);
                 match WireMsg::deserialize(message) {
                    Ok(message_type) => {
                        match message_type {
                            MessageType::NetworkInfo(msg) => {
                                debug!("SHOULD HANDLE INFRA");
                                match cm.handle_infrastructure_msg(msg).await {
                                    Ok(_) => {
                                        //do nothing
                                    },
                                    Err(error) => {
                                        error!("Error handling infra msg {:?}", error);
                                    }
                                }
                            }
                            MessageType::ClientMessage(envelope) => {
                                debug!("SHOULD HANDLE CLIENT");
        
                                cm.handle_client_msg(envelope, src).await;
                            }
                            msg_type => {
                                info!("Unexpected message type received: {:?}", msg_type);
                            }
                        }

                    },
                    Err(error) => {
                        error!("Error deserialzing message {:?}", error);
                        // do nothing else
                    }
                };

                trace!("Message received at listener from {:?}", &src);

            }
            info!("IncomingMessages listener is closing now");
            Ok::<(), Error>(())
        })
    }

    /// Handle received infrastructure messages
    async fn handle_infrastructure_msg(&self, msg: NetworkInfoMsg) -> Result<(), Error> {
        match msg {
            NetworkInfoMsg::GetSectionResponse(GetSectionResponse::SectionNetworkInfoUpdate(error)) => {
                self.handle_infrastructure_info_update(error).await
            }
            NetworkInfoMsg::GetSectionResponse(GetSectionResponse::Redirect(
                addresses,
            )) => {
                trace!("GetSectionResponse::Redirect, trying with provided elders");
                // let config = Config::new(self.config_file_path, Some(addresses.iter().cloned().collect())).qp2p;
                
                // Continually try and bootstrap against new elders while we're getting rediret
                self.get_section(&addresses).await?;
                // self.listen_to_incoming_messages(incoming).await;
                
                Ok(())
            }
            NetworkInfoMsg::GetSectionResponse(GetSectionResponse::Success(infra_info)) => {
                self.update_infrastructure_information(infra_info).await
            }
            NetworkInfoMsg::GetSectionQuery(xorname) => Err(Error::UnexpectedMessageOnJoin(
                format!("bootstrapping failed since an invalid response (NetworkInfoMsg::GetSectionQuery({})) was received", xorname)
            )),
            NetworkInfoMsg::NetworkInfoUpdate(update) => {
                let correlation_id = update.correlation_id;
                error!("MessageId {:?} was interrupted due to infrastructure updates. This will most likely need to be sent again.", correlation_id);
                if let Err(error) =  self.notification_sender.lock().await.send(Error::NetworkInfoUpdateMayHaveAffectedMsg(correlation_id)) {
                    error!("Error notifying via sender. {:?}", error);
                }
                let error = update.error;
                self.handle_infrastructure_info_update(error).await
                
            }
            _ => {
                error!("Another infrastructure message type came in {:?}", msg);
                Ok(())
            }
        }
    }

    /// Handle infrastructure udpate if possible
    async fn handle_infrastructure_info_update(
        &self,
        update: InfrastructureUpdate,
    ) -> Result<(), Error> {
        match update {
            InfrastructureUpdate::TargetSectionInfoOutdated(update) => {
                self.update_infrastructure_information(update).await
            }
            _ => {
                error!(
                    "Infrastructure update received {:?}, but was not handled",
                    update
                );
                Ok(())
            }
        }
    }

    /// update the client's elder connection information and trigger connections to those elders
    async fn update_infrastructure_information(
        &self,
        info: NetworkInfo,
    ) -> Result<(), Error> {
        let elders = info.elders;
        let pk_set = info.pk_set;

        trace!("Update Infrastructure elders: ({:?})", elders);
        // Obtain the addresses of the Elders
        let elders_addrs = elders
            .into_iter()
            .map(|(_, socket_addr)| socket_addr)
            .collect();

        // if we're waiting for inital PK set on bootstrap
        
        let mut sender = self.keyset_sender.lock().await;
            sender
                .send(Ok(pk_set))
                .await
                .map_err(|_| Error::CouldNotSaveReplicaPkSet)?;
            // let's wipe that
            // self.keyset_channel = None;
        // }

        // clear existing elder lsit.
        let mut elders = self.elders.lock().await;
        for elder in elders.clone().into_iter() {
            elders.remove(&elder);
        }
        // elders.remove;
        self.connect_to_elders(elders_addrs).await
    }

    /// Handle messages intended for client consumption (re: queries + commands)
    async fn handle_client_msg(&self, message: Message, src: SocketAddr) {
        let pending_transfer_validations = Arc::clone(&self.pending_transfer_validations);
        let notifier = self.notification_sender.clone();

        let pending_queries = self.pending_query_responses.clone();

        match message.clone() {
            Message::QueryResponse {
                response,
                correlation_id,
                ..
            } => {
                trace!("Query response in: {:?}", response);

                if let Some(mut sender) =
                    pending_queries.lock().await.remove(&(src, correlation_id))
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
                    if let Some(sender) = pending_transfer_validations
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
                if let Some(sender) = pending_transfer_validations
                    .lock()
                    .await
                    .get_mut(&correlation_id)
                {
                    debug!("Cmd Error was received, sending on channel to caller");
                    let _ = sender.send(Err(Error::from(error.clone()))).await;
                } else {
                    warn!("No sender subscribing and listening for errors relating to message {:?}. Error returned is: {:?}", correlation_id, error)
                }

                let _ = notifier.lock().await.send(Error::from(error));
            }
            msg => {
                warn!("another message type received {:?}", msg);
            }
        }
    }
}
