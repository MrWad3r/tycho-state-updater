use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tycho_types::abi::{AbiType, AbiValue, FromAbi, IntoAbi, WithAbiType};
use tycho_types::num::Tokens;
use tycho_types::prelude::HashBytes;

#[derive(Debug, Clone, Serialize, Deserialize, WithAbiType, IntoAbi, FromAbi)]
pub struct PartialElectorData {
    pub current_election: Option<Ref<CurrentElectionData>>,
    pub credits: BTreeMap<HashBytes, FpTokens>,
    pub past_elections: BTreeMap<u32, PastElectionData>,
    pub grams: Tokens,
    pub active_id: u32,
    pub active_hash: HashBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, WithAbiType, IntoAbi, FromAbi)]
pub struct CurrentElectionData {
    pub elect_at: u32,
    pub elect_close: u32,
    pub min_stake: FpTokens,
    pub total_stake: FpTokens,
    pub members: BTreeMap<HashBytes, ElectionMember>,
    pub failed: bool,
    pub finished: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, WithAbiType, IntoAbi, FromAbi)]
pub struct ElectionMember {
    pub msg_value: FpTokens,
    pub created_at: u32,
    pub stake_factor: u32,
    pub src_addr: HashBytes,
    pub adnl_addr: HashBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, WithAbiType, IntoAbi, FromAbi)]
pub struct PastElectionData {
    pub unfreeze_at: u32,
    pub stake_held: u32,
    pub vset_hash: HashBytes,
}

#[repr(transparent)]
pub struct Ref<T>(pub T);

impl<T: Clone> Clone for Ref<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Ref<T> {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Ref").field(&self.0).finish()
    }
}

impl<T: WithAbiType> WithAbiType for Ref<T> {
    fn abi_type() -> AbiType {
        AbiType::Ref(Arc::new(T::abi_type()))
    }
}

impl<T: FromAbi> FromAbi for Ref<T> {
    fn from_abi(value: AbiValue) -> Result<Self> {
        match value {
            AbiValue::Ref(value) => T::from_abi(*value).map(Self),
            value => {
                anyhow::bail!(tycho_types::abi::error::AbiError::TypeMismatch {
                    expected: Box::from("ref"),
                    ty: value.display_type().to_string().into(),
                })
            }
        }
    }
}

impl<T: IntoAbi> IntoAbi for Ref<T> {
    fn as_abi(&self) -> AbiValue {
        AbiValue::reference(self.0.as_abi())
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        AbiValue::reference(self.0.into_abi())
    }
}

impl<T: Serialize> Serialize for Ref<T> {
    #[inline]
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ref<T> {
    #[inline]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self)
    }
}

#[derive(Default, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct FpTokens(pub u128);

impl FromStr for FpTokens {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const DECIMALS: usize = 9;
        const ONE: u128 = 10u128.pow(DECIMALS as _);

        let (int, frac) = match s.split_once('.') {
            None => (s.parse::<u128>()?, 0),
            Some((int, frac)) => {
                let int = int.parse::<u128>()?;
                if frac.is_empty() || frac.len() > DECIMALS {
                    anyhow::bail!("invalid fraction part");
                }

                let leading_zeros = frac.len() - frac.trim_start_matches('0').len();
                let frac = if leading_zeros == frac.len() {
                    0
                } else {
                    let trailing_zeros = DECIMALS - frac.len();
                    frac[leading_zeros..].parse::<u128>()? * 10u128.pow(trailing_zeros as _)
                };

                (int, frac)
            }
        };
        debug_assert!(frac < ONE);

        let Some(int) = int.checked_mul(ONE) else {
            anyhow::bail!("too big integer part");
        };

        Ok(Self(int + frac))
    }
}

impl Serialize for FpTokens {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FpTokens {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(Error::custom)
    }
}

impl std::fmt::Debug for FpTokens {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for FpTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let num: u128 = self.0;
        let int = num / 1000000000;
        let mut frac = num % 1000000000;

        int.fmt(f)?;
        if frac > 0 {
            let mut len = 9usize;
            while frac.is_multiple_of(10) && frac > 0 {
                len -= 1;
                frac /= 10;
            }
            f.write_fmt(format_args!(".{frac:0len$}"))?;
        }
        Ok(())
    }
}

impl std::ops::Deref for FpTokens {
    type Target = u128;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for FpTokens {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl WithAbiType for FpTokens {
    fn abi_type() -> AbiType {
        Tokens::abi_type()
    }
}

impl IntoAbi for FpTokens {
    fn as_abi(&self) -> AbiValue {
        Tokens::from(self).into_abi()
    }

    fn into_abi(self) -> AbiValue
    where
        Self: Sized,
    {
        Tokens::from(self).into_abi()
    }
}

impl FromAbi for FpTokens {
    fn from_abi(value: AbiValue) -> Result<Self> {
        Tokens::from_abi(value).map(Self::from)
    }
}

impl From<u128> for FpTokens {
    fn from(value: u128) -> Self {
        FpTokens(value)
    }
}

impl From<Tokens> for FpTokens {
    fn from(value: Tokens) -> Self {
        FpTokens(value.into_inner())
    }
}

impl From<&Tokens> for FpTokens {
    fn from(value: &Tokens) -> Self {
        FpTokens(value.into_inner())
    }
}

impl From<FpTokens> for u128 {
    #[inline]
    fn from(value: FpTokens) -> Self {
        value.0
    }
}

impl From<FpTokens> for Tokens {
    #[inline]
    fn from(value: FpTokens) -> Self {
        Tokens::new(value.0)
    }
}

impl From<&FpTokens> for Tokens {
    #[inline]
    fn from(value: &FpTokens) -> Self {
        Tokens::new(value.0)
    }
}
