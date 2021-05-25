use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};
use std::{
    ffi::OsStr,
    fs::{self, File},
};

use bech32::{ToBase32, Variant};
use bip39::{Language, Mnemonic, Seed};
use bitcoin::{
    network::constants::Network,
    secp256k1::Secp256k1,
    util::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey},
};
use hdpath::StandardHDPath;
use ibc::ics24_host::identifier::ChainId;
use k256::ecdsa::{signature::Signer, Signature, SigningKey};
use ripemd160::Ripemd160;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use errors::{Error, Kind};
pub use pub_key::EncodedPubKey;

pub mod errors;
mod pub_key;

pub const KEYSTORE_DEFAULT_FOLDER: &str = ".hermes/keys/";
pub const KEYSTORE_DISK_BACKEND: &str = "keyring-test";
pub const KEYSTORE_FILE_EXTENSION: &str = "json";

/// [Coin type][coin-type] associated with a key.
///
/// See [the list of all registered coin types][coin-types].
///
/// [coin-type]: https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki#Coin_type
/// [coin-types]: https://github.com/satoshilabs/slips/blob/master/slip-0044.md
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CoinType(u32);

impl CoinType {
    /// Atom (Cosmos) coin type with number 118.
    pub const ATOM: CoinType = CoinType(118);

    pub fn new(coin_type: u32) -> Self {
        Self(coin_type)
    }

    pub fn num(&self) -> u32 {
        self.0
    }
}

impl Default for CoinType {
    fn default() -> Self {
        Self::ATOM
    }
}

impl FromStr for CoinType {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>().map(Self::new)
    }
}

/// Key entry stores the Private Key and Public Key as well the address
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    /// Public key
    pub public_key: ExtendedPubKey,

    /// Private key
    pub private_key: ExtendedPrivKey,

    /// Account Bech32 format - TODO allow hrp
    pub account: String,

    /// Address
    pub address: Vec<u8>,

    /// Coin type
    pub coin_type: CoinType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    pub name: String,
    pub r#type: String,
    pub address: String,
    pub pubkey: String,
    pub mnemonic: String,
    pub coin_type: Option<CoinType>,
}

impl TryFrom<KeyFile> for KeyEntry {
    type Error = Error;

    fn try_from(key_file: KeyFile) -> Result<Self, Self::Error> {
        // Decode the Bech32-encoded address from the key file
        let keyfile_address_bytes = decode_bech32(&key_file.address)?;

        let encoded_key: EncodedPubKey = key_file.pubkey.parse()?;
        let mut keyfile_pubkey_bytes = encoded_key.into_bytes();

        // Use coin type if present or default coin type (ie. Atom).
        let coin_type = key_file.coin_type.unwrap_or_default();

        // Decode the private key from the mnemonic
        let private_key = private_key_from_mnemonic(&key_file.mnemonic, coin_type)?;
        let derived_pubkey = ExtendedPubKey::from_private(&Secp256k1::new(), &private_key);
        let derived_pubkey_bytes = derived_pubkey.public_key.to_bytes();
        assert!(derived_pubkey_bytes.len() <= keyfile_pubkey_bytes.len());

        // FIXME: For some reason that is currently unclear, the public key decoded from
        //        the keyfile contains a few extraneous leading bytes. To compare both
        //        public keys, we therefore strip those leading bytes off and keep the
        //        common parts.
        let keyfile_pubkey_bytes =
            keyfile_pubkey_bytes.split_off(keyfile_pubkey_bytes.len() - derived_pubkey_bytes.len());

        // Ensure that the public key in the key file and the one extracted from the mnemonic match.
        if keyfile_pubkey_bytes != derived_pubkey_bytes {
            return Err(Kind::PublicKeyMismatch {
                keyfile: keyfile_pubkey_bytes,
                mnemonic: derived_pubkey_bytes,
            }
            .into());
        }

        Ok(KeyEntry {
            public_key: derived_pubkey,
            private_key,
            account: key_file.address,
            address: keyfile_address_bytes,
            coin_type,
        })
    }
}

pub trait KeyStore {
    fn get_key(&self, key_name: &str) -> Result<KeyEntry, Error>;
    fn add_key(&mut self, key_name: &str, key_entry: KeyEntry) -> Result<(), Error>;
    fn keys(&self) -> Result<Vec<(String, KeyEntry)>, Error>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    account_prefix: String,
    keys: HashMap<String, KeyEntry>,
}

impl Memory {
    pub fn new(account_prefix: String) -> Self {
        Self {
            account_prefix,
            keys: HashMap::new(),
        }
    }
}

impl KeyStore for Memory {
    fn get_key(&self, key_name: &str) -> Result<KeyEntry, Error> {
        self.keys
            .get(key_name)
            .cloned()
            .ok_or_else(|| Kind::KeyNotFound.into())
    }

    fn add_key(&mut self, key_name: &str, key_entry: KeyEntry) -> Result<(), Error> {
        if self.keys.contains_key(key_name) {
            return Err(Kind::ExistingKey.into());
        }

        self.keys.insert(key_name.to_string(), key_entry);

        Ok(())
    }

    fn keys(&self) -> Result<Vec<(String, KeyEntry)>, Error> {
        Ok(self
            .keys
            .iter()
            .map(|(n, k)| (n.to_string(), k.clone()))
            .collect())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Test {
    account_prefix: String,
    store: PathBuf,
}

impl Test {
    pub fn new(account_prefix: String, store: PathBuf) -> Self {
        Self {
            account_prefix,
            store,
        }
    }
}

impl KeyStore for Test {
    fn get_key(&self, key_name: &str) -> Result<KeyEntry, Error> {
        let mut key_file = self.store.join(key_name);
        key_file.set_extension(KEYSTORE_FILE_EXTENSION);

        if !key_file.as_path().exists() {
            return Err(Kind::KeyStore
                .context(format!("cannot find key file at '{}'", key_file.display()))
                .into());
        }

        let file =
            File::open(&key_file).map_err(|_| Kind::KeyStore.context("cannot open key file"))?;

        let key_entry = serde_json::from_reader(file).map_err(|_| {
            Kind::KeyStore.context(format!("cannot read key file at '{}'", key_file.display()))
        })?;

        Ok(key_entry)
    }

    fn add_key(&mut self, key_name: &str, key_entry: KeyEntry) -> Result<(), Error> {
        let mut filename = self.store.join(key_name);
        filename.set_extension(KEYSTORE_FILE_EXTENSION);

        let file = File::create(filename)
            .map_err(|_| Kind::KeyStore.context("error creating the key file"))?;

        serde_json::to_writer_pretty(file, &key_entry)
            .map_err(|_| Kind::KeyStore.context("error writing the key file"))?;

        Ok(())
    }

    fn keys(&self) -> Result<Vec<(String, KeyEntry)>, Error> {
        let dir = fs::read_dir(&self.store)
            .map_err(|e| Kind::KeyStore.context(format!("cannot list keys: {}", e)))?;

        let ext = OsStr::new(KEYSTORE_FILE_EXTENSION);

        dir.into_iter()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension() == Some(ext))
            .flat_map(|path| path.file_stem().map(OsStr::to_owned))
            .flat_map(|stem| stem.to_str().map(ToString::to_string))
            .map(|name| self.get_key(&name).map(|key| (name, key)))
            .collect()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Store {
    Memory,
    Test,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum KeyRing {
    Memory(Memory),
    Test(Test),
}

impl KeyRing {
    pub fn new(store: Store, account_prefix: &str, chain_id: &ChainId) -> Result<Self, Error> {
        match store {
            Store::Memory => Ok(Self::Memory(Memory::new(account_prefix.to_string()))),

            Store::Test => {
                let keys_folder = disk_store_path(chain_id.as_str()).map_err(|e| {
                    Kind::KeyStore.context(format!("failed to compute keys folder path: {:?}", e))
                })?;

                // Create keys folder if it does not exist
                fs::create_dir_all(&keys_folder).map_err(|e| {
                    Kind::KeyStore.context(format!("failed to create keys folder: {:?}", e))
                })?;

                Ok(Self::Test(Test::new(
                    account_prefix.to_string(),
                    keys_folder,
                )))
            }
        }
    }

    pub fn get_key(&self, key_name: &str) -> Result<KeyEntry, Error> {
        match self {
            KeyRing::Memory(m) => m.get_key(key_name),
            KeyRing::Test(d) => d.get_key(key_name),
        }
    }

    pub fn add_key(&mut self, key_name: &str, key_entry: KeyEntry) -> Result<(), Error> {
        match self {
            KeyRing::Memory(m) => m.add_key(key_name, key_entry),
            KeyRing::Test(d) => d.add_key(key_name, key_entry),
        }
    }

    pub fn keys(&self) -> Result<Vec<(String, KeyEntry)>, Error> {
        match self {
            KeyRing::Memory(m) => m.keys(),
            KeyRing::Test(d) => d.keys(),
        }
    }

    /// Get key from seed file
    pub fn key_from_seed_file(&self, key_file_content: &str) -> Result<KeyEntry, Error> {
        let key_file: KeyFile =
            serde_json::from_str(key_file_content).map_err(|e| Kind::InvalidKey.context(e))?;

        key_file.try_into()
    }

    /// Add a key entry in the store using a mnemonic.
    pub fn key_from_mnemonic(
        &self,
        mnemonic_words: &str,
        coin_type: CoinType,
    ) -> Result<KeyEntry, Error> {
        // Get the private key from the mnemonic
        let private_key = private_key_from_mnemonic(mnemonic_words, coin_type)?;

        // Get the public Key from the private key
        let public_key = ExtendedPubKey::from_private(&Secp256k1::new(), &private_key);

        // Get address from the public Key
        let address = get_address(public_key);

        // Compute Bech32 account
        let account = bech32::encode(self.account_prefix(), address.to_base32(), Variant::Bech32)
            .map_err(|e| Kind::Bech32Account.context(e))?;

        Ok(KeyEntry {
            public_key,
            private_key,
            account,
            address,
            coin_type,
        })
    }

    /// Sign a message
    pub fn sign_msg(&self, key_name: &str, msg: Vec<u8>) -> Result<Vec<u8>, Error> {
        let key = self.get_key(key_name)?;

        let private_key_bytes = key.private_key.private_key.to_bytes();
        let signing_key = SigningKey::from_bytes(private_key_bytes.as_slice()).map_err(|_| {
            Kind::InvalidKey.context("could not build signing key from private key bytes")
        })?;

        let signature: Signature = signing_key.sign(&msg);
        Ok(signature.as_ref().to_vec())
    }

    pub fn account_prefix(&self) -> &str {
        match self {
            KeyRing::Memory(m) => &m.account_prefix,
            KeyRing::Test(d) => &d.account_prefix,
        }
    }
}

/// Decode an extended private key from a mnemonic
fn private_key_from_mnemonic(
    mnemonic_words: &str,
    coin_type: CoinType,
) -> Result<ExtendedPrivKey, Error> {
    let mnemonic = Mnemonic::from_phrase(mnemonic_words, Language::English)
        .map_err(|e| Kind::InvalidMnemonic.context(e))?;

    let seed = Seed::new(&mnemonic, "");

    // Get Private Key from seed and standard derivation path
    let hd_path_format = format!("m/44'/{}'/0'/0/0", coin_type.num());
    let hd_path = StandardHDPath::try_from(hd_path_format.as_str())
        .map_err(|_| Kind::InvalidHdPath(hd_path_format))?;

    let private_key = ExtendedPrivKey::new_master(Network::Bitcoin, seed.as_bytes())
        .and_then(|k| k.derive_priv(&Secp256k1::new(), &DerivationPath::from(hd_path)))
        .map_err(|e| Kind::PrivateKey.context(e))?;

    Ok(private_key)
}

/// Return an address from a Public Key
fn get_address(pk: ExtendedPubKey) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(pk.public_key.to_bytes().as_slice());

    // Read hash digest over the public key bytes & consume hasher
    let pk_hash = hasher.finalize();

    // Plug the hash result into the next crypto hash function.
    let mut rip_hasher = Ripemd160::new();
    rip_hasher.update(pk_hash);
    let rip_result = rip_hasher.finalize();

    rip_result.to_vec()
}

fn decode_bech32(input: &str) -> Result<Vec<u8>, Error> {
    use bech32::FromBase32;

    let bytes = bech32::decode(input)
        .and_then(|(_, data, _)| Vec::from_base32(&data))
        .map_err(|e| Kind::Bech32Account.context(e))?;

    Ok(bytes)
}

fn disk_store_path(folder_name: &str) -> Result<PathBuf, Error> {
    let home = dirs_next::home_dir()
        .ok_or_else(|| Kind::KeyStore.context("cannot retrieve home folder location"))?;

    let folder = Path::new(home.as_path())
        .join(KEYSTORE_DEFAULT_FOLDER)
        .join(folder_name)
        .join(KEYSTORE_DISK_BACKEND);

    Ok(folder)
}