//! Groups: membership, detail, and join/leave.

use serde::Deserialize;

use crate::models::DataList;
use crate::{Client, Result};

const GROUPS: &str = "https://groups.roblox.com/v1";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub member_count: i64,
    pub owner: Option<GroupOwner>,
    pub shout: Option<Shout>,
    pub public_entry_allowed: bool,
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupOwner {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shout {
    pub body: String,
    pub poster: Option<GroupOwner>,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Role {
    pub id: i64,
    pub name: String,
    pub rank: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Membership {
    pub group: Group,
    pub role: Role,
}

/// Groups a user belongs to, with their role in each.
pub async fn of_user(client: &Client, user_id: i64) -> Result<Vec<Membership>> {
    let url = format!("{GROUPS}/users/{user_id}/groups/roles");
    let list: DataList<Membership> = client.get_json(&url).await?;
    Ok(list.data)
}

pub async fn get(client: &Client, group_id: i64) -> Result<Group> {
    client.get_json(&format!("{GROUPS}/groups/{group_id}")).await
}

/// Join a group.
///
/// Groups with manual approval return a 401/403 with an explanatory message
/// rather than succeeding, and groups requiring a captcha cannot be joined
/// from here at all — both surface as a normal error to the user.
pub async fn join(client: &Client, group_id: i64) -> Result<()> {
    client.post_action(&format!("{GROUPS}/groups/{group_id}/users")).await
}

/// Leave a group.
///
/// `POST .../users/{id}/leave` is **dead** — it 404s, and because Roblox routes
/// before it validates CSRF, an unauthenticated probe gets a 404 where every
/// live write answers "XSRF token invalid". Leaving is a `DELETE` on the
/// membership itself, which is the same call the site uses to remove any member.
pub async fn leave(client: &Client, group_id: i64, user_id: i64) -> Result<()> {
    client
        .delete_action(&format!("{GROUPS}/groups/{group_id}/users/{user_id}"))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_parses_the_real_response_shape() {
        let json = r#"{"data":[{
            "group":{"id":9,"name":"Turbo Builders Club","description":"Official fan club!",
                "owner":{"hasVerifiedBadge":true,"userId":156,"username":"builderman","displayName":"builderman"},
                "shout":null,"memberCount":519135,"publicEntryAllowed":true,"hasVerifiedBadge":false},
            "role":{"id":11,"name":"Owner","rank":255,"color":0}
        }]}"#;

        let list: DataList<Membership> = serde_json::from_str(json).unwrap();
        assert_eq!(list.data.len(), 1);
        assert_eq!(list.data[0].group.name, "Turbo Builders Club");
        assert_eq!(list.data[0].group.member_count, 519135);
        assert_eq!(list.data[0].role.name, "Owner");
        assert_eq!(list.data[0].role.rank, 255);
        assert_eq!(list.data[0].group.owner.as_ref().unwrap().username, "builderman");
    }

    #[test]
    fn a_null_shout_does_not_break_parsing() {
        let g: Group = serde_json::from_str(r#"{"id":1,"name":"x","shout":null}"#).unwrap();
        assert!(g.shout.is_none());
        assert_eq!(g.member_count, 0);
    }

    #[test]
    fn missing_owner_is_tolerated() {
        let g: Group = serde_json::from_str(r#"{"id":1,"name":"Ownerless"}"#).unwrap();
        assert!(g.owner.is_none());
    }
}
