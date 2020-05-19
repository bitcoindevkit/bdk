use std::cell::RefCell;
use std::collections::BTreeMap;

use bitcoin::secp256k1::Secp256k1;

use crate::descriptor::error::Error;
use crate::descriptor::keys::{parse_key, DummyKey, Key, KeyAlias, RealKey};
use crate::descriptor::{ExtendedDescriptor, MiniscriptExtractPolicy, Policy, StringDescriptor};

pub trait ParticipantType: Default {
    fn validate_aliases(aliases: Vec<&String>) -> Result<(), Error>;
}

#[derive(Default)]
pub struct Coordinator {}
impl ParticipantType for Coordinator {
    fn validate_aliases(aliases: Vec<&String>) -> Result<(), Error> {
        if aliases.into_iter().any(|a| a == "[PEER]") {
            Err(Error::InvalidAlias("[PEER]".into()))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
pub struct Peer;
impl ParticipantType for Peer {
    fn validate_aliases(aliases: Vec<&String>) -> Result<(), Error> {
        if !aliases.into_iter().any(|a| a == "[PEER]") {
            Err(Error::MissingAlias("[PEER]".into()))
        } else {
            Ok(())
        }
    }
}

pub struct Participant<T: ParticipantType> {
    descriptor: StringDescriptor,
    parsed_keys: BTreeMap<String, Box<dyn Key>>,
    received_keys: BTreeMap<String, Box<dyn RealKey>>,

    _data: T,
}

impl<T: ParticipantType> Participant<T> {
    pub fn new(sd: StringDescriptor) -> Result<Self, Error> {
        let parsed_keys = Self::parse_keys(&sd, vec![]);

        T::validate_aliases(parsed_keys.keys().collect())?;

        Ok(Participant {
            descriptor: sd,
            parsed_keys,
            received_keys: Default::default(),
            _data: Default::default(),
        })
    }

    fn parse_keys(
        sd: &StringDescriptor,
        with_secrets: Vec<&str>,
    ) -> BTreeMap<String, Box<dyn Key>> {
        let keys: RefCell<BTreeMap<String, Box<dyn Key>>> = RefCell::new(BTreeMap::new());

        let translatefpk = |string: &String| -> Result<_, Error> {
            let (key, parsed) = match parse_key(string) {
                Ok((key, parsed)) => (key, parsed.into_key()),
                Err(_) => (
                    string.clone(),
                    KeyAlias::new_boxed(string.as_str(), with_secrets.contains(&string.as_str())),
                ),
            };
            keys.borrow_mut().insert(key, parsed);

            Ok(DummyKey::default())
        };
        let translatefpkh = |string: &String| -> Result<_, Error> {
            let (key, parsed) = match parse_key(string) {
                Ok((key, parsed)) => (key, parsed.into_key()),
                Err(_) => (
                    string.clone(),
                    KeyAlias::new_boxed(string.as_str(), with_secrets.contains(&string.as_str())),
                ),
            };
            keys.borrow_mut().insert(key, parsed);

            Ok(DummyKey::default())
        };

        sd.translate_pk(translatefpk, translatefpkh).unwrap();

        keys.into_inner()
    }

    pub fn policy_for(&self, with_secrets: Vec<&str>) -> Result<Option<Policy>, Error> {
        let keys = Self::parse_keys(&self.descriptor, with_secrets);
        self.descriptor.extract_policy(&keys)
    }

    fn _missing_keys(&self) -> Vec<&String> {
        self.parsed_keys
            .keys()
            .filter(|k| !self.received_keys.contains_key(*k))
            .collect()
    }

    pub fn completed(&self) -> bool {
        self._missing_keys().is_empty()
    }

    pub fn finalize(self) -> Result<ExtendedDescriptor, Error> {
        if !self.completed() {
            return Err(Error::Incomplete);
        }

        let translatefpk = |string: &String| -> Result<_, Error> {
            Ok(format!(
                "{}",
                self.received_keys
                    .get(string)
                    .expect(&format!("Missing key: `{}`", string))
            ))
        };
        let translatefpkh = |string: &String| -> Result<_, Error> {
            Ok(format!(
                "{}",
                self.received_keys
                    .get(string)
                    .expect(&format!("Missing key: `{}`", string))
            ))
        };

        let internal = self.descriptor.translate_pk(translatefpk, translatefpkh)?;

        Ok(ExtendedDescriptor {
            internal,
            keys: self.received_keys,
            ctx: Secp256k1::gen_new(),
        })
    }
}

impl Participant<Coordinator> {
    pub fn descriptor(&self) -> &StringDescriptor {
        &self.descriptor
    }

    pub fn add_key(&mut self, alias: &str, key: Box<dyn RealKey>) -> Result<(), Error> {
        // TODO: check network

        if key.has_secret() {
            return Err(Error::KeyHasSecret);
        }

        self.received_keys.insert(alias.into(), key);

        Ok(())
    }

    pub fn received_keys(&self) -> Vec<&String> {
        self.received_keys.keys().collect()
    }

    pub fn missing_keys(&self) -> Vec<&String> {
        self._missing_keys()
    }

    pub fn descriptor_for(&self, alias: &str) -> Result<StringDescriptor, Error> {
        if !self.parsed_keys.contains_key(alias) {
            return Err(Error::MissingAlias(alias.into()));
        }

        let map_name = |s: &String| {
            if s == alias {
                "[PEER]".into()
            } else {
                s.into()
            }
        };

        let translatefpk = |string: &String| -> Result<_, Error> { Ok(map_name(string)) };
        let translatefpkh = |string: &String| -> Result<_, Error> { Ok(map_name(string)) };

        Ok(self.descriptor.translate_pk(translatefpk, translatefpkh)?)
    }

    pub fn get_map(&self) -> Result<BTreeMap<String, String>, Error> {
        if !self.completed() {
            return Err(Error::Incomplete);
        }

        Ok(self
            .received_keys
            .iter()
            .map(|(k, v)| (k.into(), format!("{}", v)))
            .collect())
    }
}

impl Participant<Peer> {
    pub fn policy(&self) -> Result<Option<Policy>, Error> {
        self.policy_for(vec!["[PEER]"])
    }

    pub fn use_key(&mut self, key: Box<dyn RealKey>) -> Result<(), Error> {
        let secp = Secp256k1::gen_new();
        self.received_keys
            .insert("[PEER]".into(), key.public(&secp)?);

        Ok(())
    }

    pub fn my_key(&mut self) -> Option<&Box<dyn RealKey>> {
        self.received_keys.get("[PEER]".into())
    }

    pub fn apply_map(mut self, map: BTreeMap<String, String>) -> Result<ExtendedDescriptor, Error> {
        let mut parsed_map: BTreeMap<_, _> = map
            .into_iter()
            .map(|(k, v)| -> Result<_, Error> {
                let (_, parsed) = parse_key(&v)?;
                Ok((k, parsed))
            })
            .collect::<Result<_, _>>()?;

        self.received_keys.append(&mut parsed_map);

        self.finalize()
    }
}
