//! Avatar and inventory — editing what the user already owns.
//!
//! There is deliberately **no catalog or marketplace surface here**: RoJoin
//! edits the avatar with items you already have and never browses or buys.
//!
//! Endpoint note, because the obvious ones are gone: per-asset
//! `avatar/assets/{id}/wear` and `avatar/set-wearing-assets` under v1 both
//! **404**. The live endpoint is `POST v2/avatar/set-wearing-assets`, which
//! takes the *complete* list of worn asset ids — wearing and removing are the
//! same call with a different list.

use serde::{Deserialize, Serialize};

use crate::models::Page;
use crate::{Client, Error, Result};

const AVATAR: &str = "https://avatar.roblox.com";
const INVENTORY: &str = "https://inventory.roblox.com";

/// The avatar categories RoJoin offers, and the Roblox asset-type ids behind
/// each. Grouped the way a person thinks about them rather than one tab per
/// numeric type.
pub const CATEGORIES: &[(&str, &[u32])] = &[
    ("Hats", &[8]),
    ("Hair", &[41]),
    ("Face", &[18, 42]),
    ("Neck", &[43]),
    ("Shoulder", &[44]),
    ("Front", &[45]),
    ("Back", &[46]),
    ("Waist", &[47]),
    ("Shirts", &[11]),
    ("Pants", &[12]),
    ("T-Shirts", &[2]),
    ("Gear", &[19]),
    ("Emotes", &[61]),
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Avatar {
    pub player_avatar_type: String,
    pub scales: Scales,
    pub body_colors: BodyColors,
    pub assets: Vec<WornAsset>,
}

impl Avatar {
    pub fn worn_ids(&self) -> Vec<i64> {
        self.assets.iter().map(|a| a.id).collect()
    }

    pub fn is_worn(&self, asset_id: i64) -> bool {
        self.assets.iter().any(|a| a.id == asset_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scales {
    pub height: f64,
    pub width: f64,
    pub head: f64,
    pub depth: f64,
    pub proportion: f64,
    pub body_type: f64,
}

impl Default for Scales {
    fn default() -> Self {
        Self { height: 1.0, width: 1.0, head: 1.0, depth: 1.0, proportion: 0.0, body_type: 0.0 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BodyColors {
    pub head_color_id: i32,
    pub torso_color_id: i32,
    pub right_arm_color_id: i32,
    pub left_arm_color_id: i32,
    pub right_leg_color_id: i32,
    pub left_leg_color_id: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WornAsset {
    pub id: i64,
    pub name: String,
    pub asset_type: AssetType,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssetType {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InventoryItem {
    pub user_asset_id: i64,
    pub asset_id: i64,
    pub asset_name: String,
    pub serial_number: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outfit {
    pub id: i64,
    pub name: String,
    pub is_editable: bool,
}

/// Anyone's avatar. Public, so it powers profile previews too.
pub async fn of_user(client: &Client, user_id: i64) -> Result<Avatar> {
    client.get_json(&format!("{AVATAR}/v1/users/{user_id}/avatar")).await
}

/// The signed-in user's avatar (authenticated).
pub async fn mine(client: &Client) -> Result<Avatar> {
    client.get_json(&format!("{AVATAR}/v1/avatar")).await
}

/// Owned items of a given asset type.
pub async fn inventory(
    client: &Client,
    user_id: i64,
    asset_type: u32,
    limit: u32,
) -> Result<Vec<InventoryItem>> {
    let limit = crate::page_limit(limit);
    let url = format!(
        "{INVENTORY}/v2/users/{user_id}/inventory/{asset_type}?limit={limit}&sortOrder=Desc"
    );
    let page: Page<InventoryItem> = client.get_json(&url).await?;
    Ok(page.data)
}

/// Everything owned across a category's asset types, de-duplicated.
pub async fn inventory_for_category(
    client: &Client,
    user_id: i64,
    category: &str,
    limit: u32,
) -> Result<Vec<InventoryItem>> {
    let types = CATEGORIES
        .iter()
        .find(|(name, _)| *name == category)
        .map(|(_, t)| *t)
        .unwrap_or(&[]);

    let mut out: Vec<InventoryItem> = Vec::new();
    for t in types {
        if let Ok(items) = inventory(client, user_id, *t, limit).await {
            for item in items {
                if !out.iter().any(|e| e.asset_id == item.asset_id) {
                    out.push(item);
                }
            }
        }
    }
    Ok(out)
}

pub async fn outfits(client: &Client, user_id: i64, per_page: u32) -> Result<Vec<Outfit>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default, deserialize_with = "crate::null_vec")]
        data: Vec<Outfit>,
    }
    let url = format!("{AVATAR}/v1/users/{user_id}/outfits?page=1&itemsPerPage={per_page}");
    let resp: Resp = client.get_json(&url).await?;
    Ok(resp.data)
}

/// An outfit's contents. There is no server-side "wear outfit" endpoint any
/// more, so wearing one means reading its assets and setting them.
pub async fn outfit_details(client: &Client, outfit_id: i64) -> Result<Avatar> {
    client.get_json(&format!("{AVATAR}/v1/outfits/{outfit_id}/details")).await
}

/// The request body `v2/avatar/set-wearing-assets` actually expects.
///
/// The v2 endpoint takes `{"assets":[{"id":N}]}`. The `{"assetIds":[N]}` shape
/// is **v1**, and sending it to v2 leaves `assets` null server-side, which
/// answers 500 rather than a validation error — so every wear and remove
/// failed identically and looked like Roblox being flaky.
///
/// `meta` (layered-clothing order and puffiness) is deliberately omitted: we
/// track worn assets by id only, so there is nothing faithful to send and the
/// server applies its defaults.
fn wear_body(asset_ids: &[i64]) -> serde_json::Value {
    serde_json::json!({
        "assets": asset_ids
            .iter()
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>(),
    })
}

/// Set the complete list of worn assets.
///
/// This is the only wear/remove mechanism: `v2/avatar/set-wearing-assets`
/// replaces what you are wearing wholesale, so callers compute the new list
/// and send it. `wear` and `remove` below are conveniences over it.
pub async fn set_wearing(client: &Client, asset_ids: &[i64]) -> Result<()> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct WearResponse {
        #[serde(default, deserialize_with = "crate::null_vec")]
        invalid_asset_ids: Vec<i64>,
        success: bool,
    }

    let resp: WearResponse = client
        .post_json(&format!("{AVATAR}/v2/avatar/set-wearing-assets"), &wear_body(asset_ids))
        .await?;

    // A 200 does not mean it worked: the endpoint reports per-asset failures in
    // the body, and silently claiming success left the UI disagreeing with the
    // account.
    if !resp.success {
        return Err(Error::Api(if resp.invalid_asset_ids.is_empty() {
            "Roblox would not accept that outfit".into()
        } else {
            format!("Roblox refused {} of those items", resp.invalid_asset_ids.len())
        }));
    }
    Ok(())
}

pub async fn wear(client: &Client, current: &[i64], asset_id: i64) -> Result<Vec<i64>> {
    let mut next = current.to_vec();
    if !next.contains(&asset_id) {
        next.push(asset_id);
    }
    set_wearing(client, &next).await?;
    Ok(next)
}

pub async fn remove(client: &Client, current: &[i64], asset_id: i64) -> Result<Vec<i64>> {
    let next: Vec<i64> = current.iter().copied().filter(|id| *id != asset_id).collect();
    set_wearing(client, &next).await?;
    Ok(next)
}

/// Wear an outfit by reading its contents and setting them.
pub async fn wear_outfit(client: &Client, outfit_id: i64) -> Result<Vec<i64>> {
    let details = outfit_details(client, outfit_id).await?;
    let ids = details.worn_ids();
    set_wearing(client, &ids).await?;
    Ok(ids)
}

pub async fn set_body_colors(client: &Client, colors: &BodyColors) -> Result<()> {
    let _: serde_json::Value = client
        .post_json(
            &format!("{AVATAR}/v1/avatar/set-body-colors"),
            &serde_json::to_value(colors)?,
        )
        .await?;
    Ok(())
}

pub async fn set_scales(client: &Client, scales: &Scales) -> Result<()> {
    let _: serde_json::Value = client
        .post_json(
            &format!("{AVATAR}/v1/avatar/set-scales"),
            &serde_json::to_value(scales)?,
        )
        .await?;
    Ok(())
}

pub async fn set_avatar_type(client: &Client, r15: bool) -> Result<()> {
    let body = serde_json::json!({ "playerAvatarType": if r15 { "R15" } else { "R6" } });
    let _: serde_json::Value = client
        .post_json(&format!("{AVATAR}/v1/avatar/set-player-avatar-type"), &body)
        .await?;
    Ok(())
}

pub async fn save_outfit(client: &Client, name: &str, avatar: &Avatar) -> Result<()> {
    // Same v2 shape as wearing, and `outfitType` is an integer here
    // (Invalid = 0, Avatar = 1, DynamicHead = 2), not the string "Avatar".
    let body = serde_json::json!({
        "name": name,
        "assets": wear_body(&avatar.worn_ids())["assets"],
        "bodyColors": avatar.body_colors,
        "scale": avatar.scales,
        "playerAvatarType": avatar.player_avatar_type,
        "outfitType": 1,
    });
    let _: serde_json::Value = client
        .post_json(&format!("{AVATAR}/v2/outfits/create"), &body)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avatar_with(ids: &[i64]) -> Avatar {
        Avatar {
            assets: ids
                .iter()
                .map(|id| WornAsset {
                    id: *id,
                    name: format!("Asset {id}"),
                    asset_type: AssetType { id: 8, name: "Hat".into() },
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn worn_ids_and_lookup() {
        let a = avatar_with(&[1, 2, 3]);
        assert_eq!(a.worn_ids(), vec![1, 2, 3]);
        assert!(a.is_worn(2));
        assert!(!a.is_worn(9));
    }

    #[test]
    fn scales_default_to_roblox_neutral_not_zero() {
        let s = Scales::default();
        assert_eq!(s.height, 1.0);
        assert_eq!(s.width, 1.0);
        assert_eq!(s.head, 1.0);
        assert_eq!(s.depth, 1.0);
    }

    #[test]
    fn parses_the_real_avatar_response() {
        let json = r#"{"scales":{"height":1.0,"width":1.0,"head":1.0,"depth":1.0,
            "proportion":0.0,"bodyType":0.0},"playerAvatarType":"R15",
            "bodyColors":{"headColorId":125,"torsoColorId":125,"rightArmColorId":125,
            "leftArmColorId":125,"rightLegColorId":125,"leftLegColorId":125},
            "assets":[{"id":11844853,"name":"Hard Hat","assetType":{"id":8,"name":"Hat"}}]}"#;

        let a: Avatar = serde_json::from_str(json).unwrap();
        assert_eq!(a.player_avatar_type, "R15");
        assert_eq!(a.body_colors.head_color_id, 125);
        assert_eq!(a.assets.len(), 1);
        assert_eq!(a.assets[0].asset_type.name, "Hat");
    }

    #[test]
    fn inventory_item_parses_the_real_shape() {
        let json = r#"{"userAssetId":1300537251543013,"assetId":82762961686618,
            "assetName":"Sakura Antlers","collectibleItemId":null,"serialNumber":null}"#;
        let i: InventoryItem = serde_json::from_str(json).unwrap();
        assert_eq!(i.asset_name, "Sakura Antlers");
        assert_eq!(i.asset_id, 82762961686618);
    }

    #[test]
    fn every_category_maps_to_at_least_one_asset_type() {
        for (name, types) in CATEGORIES {
            assert!(!types.is_empty(), "category {name} has no asset types");
        }
    }

    #[test]
    fn category_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in CATEGORIES {
            assert!(seen.insert(*name), "duplicate category {name}");
        }
    }
}

#[cfg(test)]
mod wear_body_tests {
    use super::*;

    #[test]
    fn sends_the_v2_shape_not_the_v1_one() {
        // The v1 shape (`assetIds`) makes v2 answer 500 with no explanation,
        // which is exactly how this went unnoticed. Pin the shape.
        let body = wear_body(&[1, 2]);
        assert!(body.get("assetIds").is_none(), "assetIds is the v1 shape");
        let assets = body["assets"].as_array().expect("assets must be an array");
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0]["id"], 1);
        assert_eq!(assets[1]["id"], 2);
    }

    #[test]
    fn an_empty_wardrobe_still_sends_an_array() {
        // Taking off the last item must send `[]`, not null or a missing key.
        let body = wear_body(&[]);
        assert_eq!(body["assets"], serde_json::json!([]));
    }

    #[test]
    fn order_is_preserved_so_the_server_sees_the_list_as_given() {
        let body = wear_body(&[30, 10, 20]);
        let ids: Vec<i64> =
            body["assets"].as_array().unwrap().iter().map(|a| a["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![30, 10, 20]);
    }
}
