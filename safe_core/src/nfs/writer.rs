// Copyright 2018 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::client::Client;
use crate::crypto::shared_secretbox;
use crate::errors::CoreError;
use crate::nfs::{data_map, File, NfsError, NfsFuture};
use crate::self_encryption_storage::SelfEncryptionStorage;
use crate::utils::FutureExt;
use crate::{err, ok};
use chrono::Utc;
use futures::Future;
use log::trace;
use safe_nd::Error as SndError;
use self_encryption::{DataMap, SequentialEncryptor};

/// Mode of the writer.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    /// Will create new data.
    Overwrite,
    /// Will append content to the existing data.
    Append,
}

/// Writer is used to write contents to a File and especially in chunks if the
/// file happens to be too large.
pub struct Writer<C: Client> {
    client: C,
    file: File,
    self_encryptor: SequentialEncryptor<SelfEncryptionStorage<C>>,
    encryption_key: Option<shared_secretbox::Key>,
}

impl<C: Client> Writer<C> {
    /// Create new instance of Writer.
    pub fn new(
        client: &C,
        storage: SelfEncryptionStorage<C>,
        file: File,
        mode: Mode,
        encryption_key: Option<shared_secretbox::Key>,
    ) -> Box<NfsFuture<Writer<C>>> {
        let fut = match mode {
            Mode::Append => data_map::get(client, file.data_address(), encryption_key.clone())
                .map(Some)
                .into_box(),
            Mode::Overwrite => ok!(None),
        };
        let client = client.clone();
        fut.or_else(|err| -> Box<NfsFuture<Option<DataMap>>> {
            // If the returned error is NoSuchData, fallback to OverWrite mode by returning
            // None, otherwise pass error through.
            if let NfsError::CoreError(CoreError::DataError(SndError::NoSuchData)) = err {
                ok!(None)
            } else {
                err!(err)
            }
        })
        .and_then(move |data_map| SequentialEncryptor::new(storage, data_map).map_err(From::from))
        .map(move |self_encryptor| Self {
            client,
            file,
            self_encryptor,
            encryption_key,
        })
        .map_err(From::from)
        .into_box()
    }

    /// Data of a file/blob can be written in smaller chunks.
    pub fn write(&self, data: &[u8]) -> Box<NfsFuture<()>> {
        trace!(
            "Writer writing file data of size {} into self-encryptor.",
            data.len()
        );
        self.self_encryptor
            .write(data)
            .map_err(From::from)
            .into_box()
    }

    /// close() should be invoked only after all the data is completely written. The file/blob is
    /// saved only when close() is invoked. Returns the final `File` with the data_map stored on the
    /// network.
    pub fn close(self) -> Box<NfsFuture<File>> {
        trace!("Writer induced self-encryptor close.");

        let mut file = self.file;
        let size = self.self_encryptor.len();
        let client = self.client;
        let encryption_key = self.encryption_key;
        let published = file.published();

        self.self_encryptor
            .close()
            .map_err(From::from)
            .and_then(move |(data_map, _)| {
                data_map::put(&client, &data_map, published, encryption_key)
            })
            .map(move |data_map_name| {
                file.set_data_map_name(data_map_name);
                file.set_modified_time(Utc::now());
                file.set_size(size);
                file
            })
            .into_box()
    }
}
