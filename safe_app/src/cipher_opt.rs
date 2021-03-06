// Copyright 2018 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under the MIT license <LICENSE-MIT
// https://opensource.org/licenses/MIT> or the Modified BSD license <LICENSE-BSD
// https://opensource.org/licenses/BSD-3-Clause>, at your option. This file may not be copied,
// modified, or distributed except according to those terms. Please review the Licences for the
// specific language governing permissions and limitations relating to use of the SAFE Network
// Software.

//! Cipher options.

use crate::client::AppClient;
use crate::errors::AppError;
use crate::AppContext;
use bincode::{deserialize, serialize};
use miscreant::aead::Aead;
use miscreant::aead::Aes128SivAead;
use safe_core::{utils, Client, CoreError};
use serde_derive::{Deserialize, Serialize};
use threshold_crypto::{Ciphertext, PublicKey};

/// Cipher Options
#[derive(Debug)]
pub enum CipherOpt {
    /// No encryption
    PlainText,
    /// Encrypt using symmetric keys (usually for private data)
    Symmetric,
    /// Encrypt using asymmetric encryption (encrypting for peer to read)
    Asymmetric {
        /// PublicKey of the peer to whom we want to encrypt
        peer_encrypt_key: PublicKey,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireFormat {
    Plain(Vec<u8>),
    Symmetric {
        nonce: Vec<u8>,
        cipher_text: Vec<u8>,
    },
    Asymmetric(Ciphertext),
}

impl CipherOpt {
    /// Encrypt plain text
    pub fn encrypt(&self, plain_text: &[u8], app_ctx: &AppContext) -> Result<Vec<u8>, AppError> {
        match *self {
            Self::PlainText => Ok(serialize(&WireFormat::Plain(plain_text.to_owned()))?),
            Self::Symmetric => {
                let nonce = utils::generate_nonce().to_vec();
                let sym_enc_key = app_ctx.sym_enc_key()?;
                let mut cipher = Aes128SivAead::new(&**sym_enc_key);
                let cipher_text = cipher.seal(&nonce, &[], plain_text);
                let wire_format = WireFormat::Symmetric { nonce, cipher_text };

                Ok(serialize(&wire_format)?)
            }
            Self::Asymmetric {
                ref peer_encrypt_key,
            } => {
                let cipher_text = peer_encrypt_key.encrypt(plain_text);
                Ok(serialize(&WireFormat::Asymmetric(cipher_text))?)
            }
        }
    }

    /// Decrypt something encrypted by CipherOpt::encrypt()
    pub fn decrypt(
        cipher_text: &[u8],
        app_ctx: &AppContext,
        client: &AppClient,
    ) -> Result<Vec<u8>, AppError> {
        if cipher_text.is_empty() {
            return Ok(Vec::new());
        }

        match deserialize::<WireFormat>(cipher_text)? {
            WireFormat::Plain(plain_text) => Ok(plain_text),
            WireFormat::Symmetric { nonce, cipher_text } => {
                let sym_enc_key = app_ctx.sym_enc_key()?;
                let mut cipher = Aes128SivAead::new(&**sym_enc_key);
                Ok(cipher
                    .open(&nonce, &[], &cipher_text)
                    .map_err(|_| CoreError::SymmetricDecipherFailure)?)
            }
            WireFormat::Asymmetric(cipher_text) => {
                let asym_sk = client.secret_encryption_key();
                asym_sk
                    .decrypt(&cipher_text)
                    .ok_or_else(|| AppError::from(CoreError::AsymmetricDecipherFailure))
            }
        }
    }
}
