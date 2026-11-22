//! Public character info, corporation and alliance; and the wallet balance.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DOMAINS: &[DomainSpec] = &[
    domain!("character_info", "Character Info", 21600, refresh_info),
    domain!("wallet", "Wallet", 3600, refresh_wallet),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub corporation_id: i64,
    #[serde(default)]
    pub alliance_id: Option<i64>,
    #[serde(default)]
    pub faction_id: Option<i64>,
    #[serde(default)]
    pub birthday: Option<DateTime<Utc>>,
    #[serde(default)]
    pub security_status: Option<f64>,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub race_id: i64,
    #[serde(default)]
    pub bloodline_id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub corporation: Option<Corporation>,
    #[serde(default)]
    pub alliance: Option<Alliance>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Corporation {
    pub name: String,
    #[serde(default)]
    pub ticker: String,
    #[serde(default)]
    pub member_count: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Alliance {
    pub name: String,
    #[serde(default)]
    pub ticker: String,
}

impl Info {
    pub fn race_name(&self) -> &'static str {
        match self.race_id {
            1 => "Caldari",
            2 => "Minmatar",
            4 => "Amarr",
            8 => "Gallente",
            _ => "Unknown",
        }
    }
}

async fn refresh_info(cx: &Cx) -> DomainResult {
    let mut info: Info = cx.esi().get(format!("characters/{}", cx.id)).json().await?;
    info.corporation = cx.esi().get(format!("corporations/{}", info.corporation_id)).json().await.ok();
    if let Some(a) = info.alliance_id {
        info.alliance = cx.esi().get(format!("alliances/{a}")).json().await.ok();
    }
    Ok(Box::new(move |c| c.info = Some(info)))
}

async fn refresh_wallet(cx: &Cx) -> DomainResult {
    let balance: f64 = cx.get(cx.char_path("wallet")).scope("esi-wallet.read_character_wallet.v1").json().await?;
    Ok(Box::new(move |c| c.wallet = Some(balance)))
}
