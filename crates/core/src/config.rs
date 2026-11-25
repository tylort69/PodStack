//! Fixed application settings: EVE SSO/ESI endpoints, the scopes PodStack asks for, and refresh intervals.

pub const APP_NAME: &str = "PodStack";
pub const APP_ID: &str = "io.github.tylort69.PodStack";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn user_agent() -> String {
    format!("podstack-native/{VERSION} (+https://github.com/tylort69/PodStack)")
}

pub const SSO_BASE: &str = "https://login.eveonline.com/v2/oauth";
pub const SSO_REVOKE_URL: &str = "https://login.eveonline.com/v2/oauth/revoke";

/// PodStack's application on developers.eveonline.com (public client: the login uses PKCE, no secret). Users can
/// register their own application and set its client id and callback in Settings → EVE login.
pub const DEFAULT_CLIENT_ID: &str = "113d91f366b143b482676d8dcf517d1f";
/// The callback registered for [`DEFAULT_CLIENT_ID`]. Nothing listens there: after logging in the browser shows an
/// error page at this address and the user pastes that address back into PodStack. A client of the user's own with a
/// loopback callback (`http://127.0.0.1:<port>/callback`) is picked up automatically instead.
pub const DEFAULT_CALLBACK: &str = "https://localhost/callback";
/// The application inherited from Cerebral; refresh tokens it issued only work with it.
pub const LEGACY_CLIENT_ID: &str = "c9fde897cdfb45208fb5254e3ee98d19";

pub const ESI_BASE: &str = "https://esi.evetech.net";
pub const ESI_TENANT: &str = "tranquility";
/// ESI behaviour is pinned to this date, see https://esi.evetech.net/meta/compatibility-dates
pub const ESI_COMPATIBILITY_DATE: &str = "2026-08-18";

/// Seconds between uses of the "Refresh from ESI" button per character.
pub const MANUAL_REFRESH_COOLDOWN: i64 = 300;
/// Buying a skill from the in-game skill window costs its NPC base price plus this markup.
pub const SKILL_WINDOW_MARKUP: f64 = 1.3;

pub struct ScopeInfo {
    pub name: &'static str,
    pub description: &'static str,
}

pub const SCOPES: &[ScopeInfo] = &[
    ScopeInfo { name: "esi-location.read_location.v1", description: "Read Current Location" },
    ScopeInfo { name: "esi-location.read_ship_type.v1", description: "Read Active Ship Type" },
    ScopeInfo { name: "esi-location.read_online.v1", description: "Read Online Status" },
    ScopeInfo { name: "esi-skills.read_skills.v1", description: "Read Skills" },
    ScopeInfo { name: "esi-skills.read_skillqueue.v1", description: "Read Skill Queue" },
    ScopeInfo { name: "esi-clones.read_clones.v1", description: "Read Jump Clones" },
    ScopeInfo { name: "esi-clones.read_implants.v1", description: "Read Active Implants" },
    ScopeInfo { name: "esi-wallet.read_character_wallet.v1", description: "Read Wallet" },
    ScopeInfo { name: "esi-mail.read_mail.v1", description: "Read Mail" },
    ScopeInfo { name: "esi-universe.read_structures.v1", description: "Read Structures" },
    ScopeInfo { name: "esi-search.search_structures.v1", description: "Search Structures" },
    ScopeInfo { name: "esi-fittings.read_fittings.v1", description: "Read Fittings" },
    ScopeInfo { name: "esi-characters.read_fatigue.v1", description: "Read Jump Fatigue" },
    ScopeInfo { name: "esi-characters.read_loyalty.v1", description: "Read Loyalty Points" },
    ScopeInfo { name: "esi-contracts.read_character_contracts.v1", description: "Read Contracts" },
    ScopeInfo { name: "esi-contracts.read_corporation_contracts.v1", description: "Read Corporation Contracts" },
    ScopeInfo { name: "esi-assets.read_assets.v1", description: "Read Assets" },
    ScopeInfo { name: "esi-industry.read_character_jobs.v1", description: "Read Industry Jobs" },
    ScopeInfo { name: "esi-characters.read_blueprints.v1", description: "Read Blueprints" },
    ScopeInfo { name: "esi-planets.manage_planets.v1", description: "Read Planetary Colonies" },
    ScopeInfo { name: "esi-markets.read_character_orders.v1", description: "Read Market Orders" },
    ScopeInfo { name: "esi-characters.read_standings.v1", description: "Read Standings" },
    ScopeInfo { name: "esi-characters.read_agents_research.v1", description: "Read Research Agents" },
    ScopeInfo { name: "esi-characters.read_notifications.v1", description: "Read Notifications" },
    ScopeInfo { name: "esi-calendar.read_calendar_events.v1", description: "Read Calendar" },
    ScopeInfo { name: "esi-killmails.read_killmails.v1", description: "Read Killmails" },
    ScopeInfo { name: "esi-characters.read_medals.v1", description: "Read Medals" },
    ScopeInfo { name: "esi-characters.read_contacts.v1", description: "Read Contacts" },
    ScopeInfo { name: "esi-characters.read_fw_stats.v1", description: "Read Factional Warfare Stats" },
];

pub fn scope_names() -> Vec<&'static str> {
    SCOPES.iter().map(|s| s.name).collect()
}

pub const IMAGE_SERVER: &str = "https://images.evetech.net";

pub fn character_portrait(id: i64, size: u32) -> String {
    format!("{IMAGE_SERVER}/characters/{id}/portrait?size={size}")
}
pub fn corporation_logo(id: i64, size: u32) -> String {
    format!("{IMAGE_SERVER}/corporations/{id}/logo?size={size}")
}
pub fn alliance_logo(id: i64, size: u32) -> String {
    format!("{IMAGE_SERVER}/alliances/{id}/logo?size={size}")
}
pub fn type_icon(id: i64, size: u32) -> String {
    format!("{IMAGE_SERVER}/types/{id}/icon?size={size}")
}
pub fn type_render(id: i64, size: u32) -> String {
    format!("{IMAGE_SERVER}/types/{id}/render?size={size}")
}
