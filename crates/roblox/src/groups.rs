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
    /// Only the group's own roles listing carries this; a membership record
    /// does not, so it is 0 there.
    #[serde(default)]
    pub member_count: i64,
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

/// The group's rank ladder, highest rank first.
///
/// One cheap call that also carries each role's member count, which is worth
/// showing on its own — and it is what makes a genuinely rank-ordered member
/// list possible.
pub async fn roles(client: &Client, group_id: i64) -> Result<Vec<Role>> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct Resp {
        #[serde(deserialize_with = "crate::null_vec")]
        roles: Vec<Role>,
    }

    let url = format!("{GROUPS}/groups/{group_id}/roles");
    let mut resp: Resp = client.get_json(&url).await?;
    resp.roles.sort_by_key(|r| std::cmp::Reverse(r.rank));
    Ok(resp.roles)
}

/// Members holding one specific role.
///
/// Listing `/groups/{id}/users` instead returns members in no particular order,
/// so sorting one page of a few thousand by rank would be sorting an arbitrary
/// sample and calling it a ranking. Walking the ladder from the top gives a list
/// that really is ordered by rank.
pub async fn role_members(
    client: &Client,
    group_id: i64,
    role_id: i64,
    limit: u32,
) -> Result<Vec<crate::models::UserSearchResult>> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct Row {
        user_id: i64,
        username: String,
        display_name: String,
        has_verified_badge: bool,
    }

    let limit = crate::page_limit(limit);
    let url = format!(
        "{GROUPS}/groups/{group_id}/roles/{role_id}/users?limit={limit}&sortOrder=Asc"
    );
    let page: crate::models::Page<Row> = client.get_json(&url).await?;

    Ok(page
        .data
        .into_iter()
        .map(|r| crate::models::UserSearchResult {
            id: r.user_id,
            name: r.username,
            display_name: r.display_name,
            has_verified_badge: r.has_verified_badge,
        })
        .collect())
}
