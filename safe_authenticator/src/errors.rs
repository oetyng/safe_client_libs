// Copyright 2018 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

//! Errors thrown by Authenticator routines.

use bincode::Error as SerialisationError;
use ffi_utils::StringError;
use futures::sync::mpsc::SendError;
use safe_core::ipc::IpcError;
use safe_core::nfs::NfsError;
use safe_core::CoreError;
use safe_nd::Error as SndError;
use std::error::Error;
use std::ffi::NulError;
use std::fmt::{self, Display, Formatter};
use std::io::Error as IoError;
use std::str::Utf8Error;
use std::string::FromUtf8Error;
use std::sync::mpsc::RecvError;

/// Authenticator errors.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AuthError {
    /// Unexpected - probably a logic error.
    Unexpected(String),
    /// Error from safe_core.
    CoreError(CoreError),
    /// Error from safe-nd
    SndError(SndError),
    /// Input/output error.
    IoError(IoError),
    /// NFS error
    NfsError(NfsError),
    /// Serialisation error.
    EncodeDecodeError,
    /// IPC error.
    IpcError(IpcError),

    /// Failure during the creation of standard account containers.
    AccountContainersCreation(String),
    /// Failure due to the attempted creation of an invalid container.
    NoSuchContainer(String),
    /// Couldn't authenticate app that is pending revocation.
    PendingRevocation,
}

impl Display for AuthError {
    fn fmt(&self, formatter: &mut Formatter) -> fmt::Result {
        match *self {
            Self::Unexpected(ref error) => {
                write!(formatter, "Unexpected (probably a logic error): {}", error)
            }
            Self::CoreError(ref error) => write!(formatter, "Core error: {}", error),
            Self::SndError(ref error) => write!(formatter, "Safe ND error: {}", error),
            Self::IoError(ref error) => write!(formatter, "I/O error: {}", error),
            Self::NfsError(ref error) => write!(formatter, "NFS error: {:?}", error),
            Self::EncodeDecodeError => write!(formatter, "Serialisation error"),
            Self::IpcError(ref error) => write!(formatter, "IPC error: {:?}", error),

            Self::AccountContainersCreation(ref reason) => write!(
                formatter,
                "Account containers creation error: {}. Login to attempt recovery.",
                reason
            ),
            Self::NoSuchContainer(ref name) => {
                write!(formatter, "'{}' not found in the access container", name)
            }
            Self::PendingRevocation => write!(
                formatter,
                "Couldn't authenticate app that is pending revocation"
            ),
        }
    }
}

impl Into<IpcError> for AuthError {
    fn into(self) -> IpcError {
        match self {
            Self::Unexpected(desc) => IpcError::Unexpected(desc),
            Self::IpcError(err) => err,
            err => IpcError::Unexpected(format!("{:?}", err)),
        }
    }
}

impl<T: 'static> From<SendError<T>> for AuthError {
    fn from(error: SendError<T>) -> Self {
        Self::Unexpected(error.description().to_owned())
    }
}

impl From<CoreError> for AuthError {
    fn from(error: CoreError) -> Self {
        Self::CoreError(error)
    }
}

impl From<IpcError> for AuthError {
    fn from(error: IpcError) -> Self {
        Self::IpcError(error)
    }
}

impl From<RecvError> for AuthError {
    fn from(error: RecvError) -> Self {
        Self::from(error.description())
    }
}

impl From<NulError> for AuthError {
    fn from(error: NulError) -> Self {
        Self::from(error.description())
    }
}

impl From<IoError> for AuthError {
    fn from(error: IoError) -> Self {
        Self::IoError(error)
    }
}

impl From<SndError> for AuthError {
    fn from(error: SndError) -> Self {
        Self::SndError(error)
    }
}

impl<'a> From<&'a str> for AuthError {
    fn from(error: &'a str) -> Self {
        Self::Unexpected(error.to_owned())
    }
}

impl From<String> for AuthError {
    fn from(error: String) -> Self {
        Self::Unexpected(error)
    }
}

impl From<NfsError> for AuthError {
    fn from(error: NfsError) -> Self {
        Self::NfsError(error)
    }
}

impl From<SerialisationError> for AuthError {
    fn from(_err: SerialisationError) -> Self {
        Self::EncodeDecodeError
    }
}

impl From<Utf8Error> for AuthError {
    fn from(_err: Utf8Error) -> Self {
        Self::EncodeDecodeError
    }
}

impl From<FromUtf8Error> for AuthError {
    fn from(_err: FromUtf8Error) -> Self {
        Self::EncodeDecodeError
    }
}

impl From<StringError> for AuthError {
    fn from(_err: StringError) -> Self {
        Self::EncodeDecodeError
    }
}
