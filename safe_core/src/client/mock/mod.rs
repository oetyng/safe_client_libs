// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

pub mod vault;

mod account;
mod connection_manager;
#[cfg(test)]
mod tests;

pub use self::account::{Account, CoinBalance};
pub use self::connection_manager::{ConnectionManager, RequestHookFn};
use safe_nd::{Address, IDataAddress, MDataAddress};
use serde::{Deserialize, Serialize};

/// Identifier for a data.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub enum DataId {
    /// Identifier of immutable data.
    Immutable(IDataAddress),
    /// Identifier of mutable data.
    Mutable(MDataAddress),
    /// Identifier of Sequence.
    Sequence(Address),
}
