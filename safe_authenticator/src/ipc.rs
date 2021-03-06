// Copyright 2018 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

//! Inter-Process Communication utilities.

use super::{AuthError, AuthFuture};
use crate::app_auth::{app_state, AppState};
use crate::client::AuthClient;
use crate::config;
use crate::ffi::errors::Error as FFIError;
use bincode::deserialize;
use ffi_utils::ffi_error;
use ffi_utils::StringError;
use futures::future::{self, Either};
use futures::Future;
use log::trace;
use safe_core::core_structs::{UserMetadata, METADATA_KEY};
use safe_core::ffi::ipc::resp::MetadataResponse as FfiUserMetadata;
use safe_core::ipc::req::{IpcReq, ShareMDataReq};
use safe_core::ipc::resp::IpcResp;
use safe_core::ipc::{self, IpcError, IpcMsg};
use safe_core::{err, ok};
use safe_core::{Client, CoreError, FutureExt};
use safe_nd::{Error as SndError, XorName};
use std::ffi::CString;

/// Decodes a given encoded IPC message and returns either an `IpcMsg` struct or
/// an error code + description & an encoded `IpcMsg::Resp` in case of an error
#[allow(clippy::type_complexity)]
pub fn decode_ipc_msg(
    client: &AuthClient,
    msg: IpcMsg,
) -> Box<AuthFuture<Result<IpcMsg, (i32, String, CString)>>> {
    match msg {
        IpcMsg::Req {
            request: IpcReq::Auth(auth_req),
            req_id,
        } => {
            // Ok status should be returned for all app states (including
            // Revoked and Authenticated).
            ok!(Ok(IpcMsg::Req {
                req_id,
                request: IpcReq::Auth(auth_req),
            }))
        }
        IpcMsg::Req {
            request: IpcReq::Unregistered(extra_data),
            req_id,
        } => ok!(Ok(IpcMsg::Req {
            req_id,
            request: IpcReq::Unregistered(extra_data),
        })),
        IpcMsg::Req {
            request: IpcReq::ShareMData(share_mdata_req),
            req_id,
        } => ok!(Ok(IpcMsg::Req {
            req_id,
            request: IpcReq::ShareMData(share_mdata_req),
        })),
        IpcMsg::Req {
            request: IpcReq::Containers(cont_req),
            req_id,
        } => {
            trace!("Handling IpcReq::Containers({:?})", cont_req);

            let app_id = cont_req.app.id.clone();
            let c2 = client.clone();

            config::list_apps(client)
                .and_then(move |(_config_version, config)| app_state(&c2, &config, &app_id))
                .and_then(move |app_state| {
                    match app_state {
                        AppState::Authenticated => Ok(Ok(IpcMsg::Req {
                            req_id,
                            request: IpcReq::Containers(cont_req),
                        })),
                        AppState::Revoked | AppState::NotAuthenticated => {
                            // App is not authenticated
                            let (error_code, description) =
                                ffi_error!(FFIError::from(IpcError::UnknownApp));

                            let response = IpcMsg::Resp {
                                response: IpcResp::Auth(Err(IpcError::UnknownApp)),
                                req_id,
                            };
                            let encoded_response = encode_response(&response)?;

                            Ok(Err((error_code, description, encoded_response)))
                        }
                    }
                })
                .into_box()
        }
        IpcMsg::Resp { .. } | IpcMsg::Revoked { .. } | IpcMsg::Err(..) => {
            return err!(AuthError::IpcError(IpcError::InvalidMsg));
        }
    }
}

/// Encode `IpcMsg` into a `CString`, using base32 encoding.
pub fn encode_response(msg: &IpcMsg) -> Result<CString, IpcError> {
    let response = ipc::encode_msg(msg)?;
    Ok(CString::new(response).map_err(StringError::from)?)
}

enum ShareMDataError {
    InvalidOwner(XorName, u64),
    InvalidMetadata,
}

/// Decodes the `ShareMData` IPC request, returning a list of `UserMetadata`.
pub fn decode_share_mdata_req(
    client: &AuthClient,
    req: &ShareMDataReq,
) -> Box<AuthFuture<Vec<Option<FfiUserMetadata>>>> {
    let user = client.public_key();
    let num_mdata = req.mdata.len();
    let mut futures = Vec::with_capacity(num_mdata);

    for mdata in &req.mdata {
        let client = client.clone();
        let name = mdata.name;
        let type_tag = mdata.type_tag;

        let future = client
            .get_seq_mdata_shell(name, type_tag)
            .and_then(move |shell| {
                if *shell.owner() == user {
                    let future_metadata = client
                        .get_seq_mdata_value(name, type_tag, METADATA_KEY.into())
                        .then(move |res| match res {
                            Ok(value) => Ok(deserialize::<UserMetadata>(&value.data)
                                .map_err(|_| ShareMDataError::InvalidMetadata)
                                .and_then(move |metadata| {
                                    match metadata.into_md_response(name, type_tag) {
                                        Ok(meta) => Ok(meta),
                                        Err(_) => Err(ShareMDataError::InvalidMetadata),
                                    }
                                })),
                            Err(CoreError::DataError(SndError::NoSuchEntry)) => {
                                // Allow requesting shared access to arbitrary Mutable Data objects even
                                // if they don't have metadata.
                                let user_metadata = UserMetadata {
                                    name: None,
                                    description: None,
                                };
                                let user_md_response = user_metadata
                                    .into_md_response(name, type_tag)
                                    .map_err(|_| ShareMDataError::InvalidMetadata);
                                Ok(user_md_response)
                            }
                            Err(error) => Err(error),
                        });
                    Either::A(future_metadata)
                } else {
                    Either::B(future::ok(Err(ShareMDataError::InvalidOwner(
                        name, type_tag,
                    ))))
                }
            })
            .map_err(AuthError::from);

        futures.push(future);
    }

    future::join_all(futures)
        .and_then(move |results| {
            let mut metadata_cont = Vec::with_capacity(num_mdata);
            let mut invalids = Vec::with_capacity(num_mdata);

            for result in results {
                match result {
                    Ok(metadata) => metadata_cont.push(Some(metadata)),
                    Err(ShareMDataError::InvalidMetadata) => metadata_cont.push(None),
                    Err(ShareMDataError::InvalidOwner(name, type_tag)) => {
                        invalids.push((name, type_tag))
                    }
                }
            }

            if invalids.is_empty() {
                Ok(metadata_cont)
            } else {
                Err(AuthError::IpcError(IpcError::InvalidOwner(invalids)))
            }
        })
        .into_box()
}
