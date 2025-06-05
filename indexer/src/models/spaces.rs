use indexer_utils::{id::derive_space_id, network_ids::GEO};
use uuid::Uuid;

use crate::CreatedSpace;

#[derive(Clone)]
pub enum SpaceType {
    Personal,
    Public,
}

#[derive(Clone)]
pub struct SpaceItem {
    pub id: Uuid,
    pub space_type: SpaceType,
    pub dao_address: String,
    pub space_address: String,
    pub voting_address: Option<String>,
    pub membership_address: Option<String>,
    pub personal_address: Option<String>,
}

pub struct SpacesModel;

impl SpacesModel {
    pub fn map_created_spaces(spaces: &Vec<CreatedSpace>) -> Vec<SpaceItem> {
        let mut created_spaces = Vec::new();

        for space in spaces {
            let space_item = match space {
                CreatedSpace::Personal(personal) => {
                    let space_id = derive_space_id(GEO, &personal.dao_address);
                    SpaceItem {
                        id: space_id,
                        space_type: SpaceType::Personal,
                        dao_address: personal.dao_address.clone(),
                        space_address: personal.space_address.clone(),
                        voting_address: None,
                        membership_address: None,
                        personal_address: Some(personal.personal_plugin.clone()),
                    }
                }
                CreatedSpace::Public(public) => {
                    let space_id = derive_space_id(GEO, &public.dao_address);
                    SpaceItem {
                        id: space_id,
                        space_type: SpaceType::Public,
                        dao_address: public.dao_address.clone(),
                        space_address: public.space_address.clone(),
                        voting_address: Some(public.governance_plugin.clone()),
                        membership_address: Some(public.membership_plugin.clone()),
                        personal_address: None,
                    }
                }
            };

            created_spaces.push(space_item);
        }

        return created_spaces;
    }
}
