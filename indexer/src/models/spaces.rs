use indexer_utils::{id::derive_space_id, network_ids::GEO};
use uuid::Uuid;

use crate::CreatedSpace;

#[derive(Clone)]
pub struct SpaceItem {
    pub id: Uuid,
    pub dao_address: String,
}

pub struct SpacesModel;

impl SpacesModel {
    pub fn map_created_spaces(spaces: &Vec<CreatedSpace>) -> Vec<SpaceItem> {
        let mut created_spaces = Vec::new();

        for space in spaces {
            let dao_address = match space {
                CreatedSpace::Personal(personal) => personal.dao_address.clone(),
                CreatedSpace::Public(public) => public.dao_address.clone(),
            };

            let space_id = derive_space_id(GEO, &dao_address);

            created_spaces.push(SpaceItem {
                id: space_id,
                dao_address,
            });
        }

        return created_spaces;
    }
}
