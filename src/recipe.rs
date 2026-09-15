use crate::api;
use crate::config;
use crate::gw2efficiency;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum RecipeSource {
    Automatic,
    Discoverable,
    Purchasable,
    Achievement,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: Option<u32>,
    pub output_item_id: u32,
    pub output_item_count: u32,
    pub disciplines: Vec<config::Discipline>,
    pub ingredients: Vec<api::RecipeIngredient>,
    source: RecipeSource,
}

impl From<api::Recipe> for Recipe {
    fn from(recipe: api::Recipe) -> Self {
        let source = if recipe.is_purchased() {
            RecipeSource::Purchasable
        } else if recipe.is_automatic() {
            RecipeSource::Automatic
        } else {
            RecipeSource::Discoverable
        };
        Recipe {
            id: Some(recipe.id),
            output_item_id: recipe.output_item_id,
            output_item_count: recipe.output_item_count,
            disciplines: recipe.disciplines,
            ingredients: recipe.ingredients,
            source,
        }
    }
}

impl TryFrom<gw2efficiency::Recipe> for Recipe {
    type Error = String;

    fn try_from(recipe: gw2efficiency::Recipe) -> Result<Self, Self::Error> {
        let output_item_count = if let Some(count) = recipe.output_item_count {
            count
        } else {
            return Err(format!(
                "Ignoring '{}'. Failed to parse 'output_item_count' as integer.",
                recipe.name
            ));
        };
        // Any disciplines _except_ Achievement can be counted as known
        // While some regular discipline precursor recipes must be learned, the
        // outputs appear to be account bound anyway, so won't be on TP.
        // There are some useful Scribe WvW BPs in the data, so ignoring all
        // normal discipline recipes would catch those too.
        let source = if recipe
            .disciplines
            .contains(&config::Discipline::Achievement)
        {
            RecipeSource::Achievement
        } else {
            RecipeSource::Automatic
        };
        Ok(Recipe {
            id: None,
            output_item_id: recipe.output_item_id,
            output_item_count,
            disciplines: recipe.disciplines,
            ingredients: recipe.ingredients,
            source,
        })
    }
}

impl Recipe {
    // see https://wiki.guildwars2.com/wiki/Category:Time_gated_recipes
    // for a list of time gated recipes
    // I've left Charged Quartz Crystals off the list, since they can
    // drop from containers.
    pub fn is_timegated(&self) -> bool {
        self.output_item_id == 46740         // Spool of Silk Weaving Thread
            || self.output_item_id == 46742  // Lump of Mithrillium
            || self.output_item_id == 46744  // Glob of Elder Spirit Residue
            || self.output_item_id == 46745  // Spool of Thick Elonian Cord
            || self.output_item_id == 66913  // Clay Pot
            || self.output_item_id == 66917  // Plate of Meaty Plant Food
            || self.output_item_id == 66923  // Plate of Piquant Plan Food
            || self.output_item_id == 67015  // Heat Stone
            || self.output_item_id == 67377  // Vial of Maize Balm
            || self.output_item_id == 79726  // Dragon Hatchling Doll Eye
            || self.output_item_id == 79763  // Gossamer Stuffing
            || self.output_item_id == 79790  // Dragon Hatchling Doll Hide
            || self.output_item_id == 79795  // Dragon Hatchling Doll Adornments
            || self.output_item_id == 79817  // Dragon Hatchling Doll Frame
            || self.output_item_id == 43772 // Charged Quartz Crystal
    }

    pub fn is_automatic(&self) -> bool {
        match &self.source {
            RecipeSource::Purchasable | RecipeSource::Achievement => false,
            // These aren't included in the API; assume you know them
            RecipeSource::Automatic | RecipeSource::Discoverable => true,
            // TODO: instead, check if account has a char with the required crafting level
            // Would require a key with the characters scope. Still wouldn't detect
            // discoverable recipes, but would detect access to them
        }
    }

    pub fn sorted_ingredients(&self) -> Vec<&api::RecipeIngredient> {
        let mut ingredients: Vec<&api::RecipeIngredient> = self.ingredients.iter().collect();
        ingredients.sort_unstable_by(|a, b| match b.count.cmp(&a.count) {
            Ordering::Equal => b.item_id.cmp(&a.item_id),
            v => v,
        });
        ingredients
    }

    pub fn collect_ingredient_ids(&self, recipes_map: &HashMap<u32, Recipe>, ids: &mut Vec<u32>) {
        for ingredient in &self.ingredients {
            if ids.contains(&ingredient.item_id) {
                continue;
            }
            ids.push(ingredient.item_id);
            if let Some(recipe) = recipes_map.get(&ingredient.item_id) {
                recipe.collect_ingredient_ids(recipes_map, ids);
            }
        }
    }

    pub fn collect_unknown_recipe_ids(
        &self,
        recipes_map: &HashMap<u32, Recipe>,
        known_recipes: &Option<HashSet<u32>>,
        unknown_recipes: &mut HashSet<u32>,
    ) {
        if let Some(id) = self.id.filter(|id| !unknown_recipes.contains(id)) {
            if !self.is_automatic() {
                // If we have no known recipes, assume we know none
                if known_recipes
                    .as_ref()
                    .filter(|recipes| recipes.contains(&id))
                    .is_none()
                {
                    unknown_recipes.insert(id);
                }
            }
        }

        for ingredient in &self.ingredients {
            if let Some(recipe) = recipes_map.get(&ingredient.item_id) {
                recipe.collect_unknown_recipe_ids(&recipes_map, &known_recipes, unknown_recipes);
            }
        }
    }

    pub fn additional_recipes() -> Vec<Recipe> {
        vec![
            // Piece of Dragon Jade
            Recipe {
                id: None,
                output_item_id: 97487u32,
                output_item_count: 1,
                disciplines: vec![
                    config::Discipline::Armorsmith,
                    config::Discipline::Artificer,
                    config::Discipline::Huntsman,
                    config::Discipline::Jeweler,
                    config::Discipline::Leatherworker,
                    config::Discipline::Tailor,
                    config::Discipline::Weaponsmith,
                ],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 97102u32,
                        count: 4,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 30,
                    },
                    api::RecipeIngredient {
                        item_id: 19721u32,
                        count: 2,
                    },
                    api::RecipeIngredient {
                        item_id: 19685u32,
                        count: 5,
                    },
                ],
                source: RecipeSource::Automatic,
            },
            // Jade Bot Core Tier 1-10
            Recipe {
                id: None,
                output_item_id: 97339u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 3,
                    },
                    api::RecipeIngredient {
                        item_id: 19679u32,
                        count: 5,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 97041u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 97339u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 10,
                    },
                    api::RecipeIngredient {
                        item_id: 19680u32,
                        count: 10,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 97284u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 97041u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 20,
                    },
                    api::RecipeIngredient {
                        item_id: 19683u32,
                        count: 25,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 96628u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 97284u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 30,
                    },
                    api::RecipeIngredient {
                        item_id: 19687u32,
                        count: 25,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 95864u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 96628u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 50,
                    },
                    api::RecipeIngredient {
                        item_id: 19688u32,
                        count: 25,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 96467u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 95864u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 80,
                    },
                    api::RecipeIngredient {
                        item_id: 19682u32,
                        count: 25,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 97020u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 96467u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 130,
                    },
                    api::RecipeIngredient {
                        item_id: 19686u32,
                        count: 25,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 96299u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 97020u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 210,
                    },
                    api::RecipeIngredient {
                        item_id: 19684u32,
                        count: 50,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 96070u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 96299u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 340,
                    },
                    api::RecipeIngredient {
                        item_id: 19685u32,
                        count: 15,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 1,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            Recipe {
                id: None,
                output_item_id: 96613u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Jeweler],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 96070u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 96052u32,
                        count: 550,
                    },
                    api::RecipeIngredient {
                        item_id: 46743u32,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: 97487u32,
                        count: 2,
                    },
                ],
                source: RecipeSource::Purchasable,
            },
            // TODO: general vendor interface, for money+item purchases, so I can avoid faking it
            // w/currency items
            // Bottle of Coconut Milk
            Recipe {
                id: None,
                output_item_id: 87289u32,
                output_item_count: 1,
                disciplines: vec![config::Discipline::Merchant],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: 38030u32, // karma
                        count: 249,
                    },
                    api::RecipeIngredient {
                        item_id: 86977u32, // Difluorite crystal
                        count: 1,
                    },
                ],
                source: RecipeSource::Automatic,
            },
        ]
    }

    /// Mystic Forge material promotions (tier upgrades), verified against
    /// https://wiki.guildwars2.com/wiki/Mystic_Forge (see the per-material
    /// pages, e.g. Mystic Crystal/Material promotion recipes). Fixed-quantity
    /// binding-agent variants only: 8/6/4/2 base -> x4/x3/x2/x1 output.
    /// The wine variants are strictly dominated (same 2x base rate plus a
    /// 2504c bottle of wine), so only the cheaper agent path is listed.
    /// Probabilistic forge recipes (clovers, bulk promotions, lotteries)
    /// are deliberately excluded: the profit math needs fixed outputs.
    /// Mystic Crystals / Binding Agents are account-bound vendor goods
    /// counted as free (see Item::token_value), like karma basics.
    pub fn mystic_forge_recipes() -> Vec<Recipe> {
        const CRYSTAL: u32 = 20799; // Mystic Crystal
        const AGENT: u32 = 39125; // Mystic Binding Agent
                                  // (output, base, dust, agent output count; agent input is
                                  // 8/6/4/2x base for x4/x3/x2/x1 outputs respectively)
        const RADIANT: u32 = 24274;
        const LUMINOUS: u32 = 24275;
        const INCANDESCENT: u32 = 24276;
        const CRYSTALLINE: u32 = 24277;
        let steps: &[(u32, u32, u32, u32, u32)] = &[
            (24302, 24301, RADIANT, 8, 4),      // Charged Fragment x4
            (24303, 24302, LUMINOUS, 6, 3),     // Charged Shard x3
            (24304, 24303, INCANDESCENT, 4, 2), // Charged Core x2
            (24305, 24304, CRYSTALLINE, 2, 1),  // Charged Lodestone x1
            (24337, 24336, RADIANT, 8, 4),      // Corrupted Fragment x4
            (24338, 24337, LUMINOUS, 6, 3),     // Corrupted Shard x3
            (24339, 24338, INCANDESCENT, 4, 2), // Corrupted Core x2
            (24340, 24339, CRYSTALLINE, 2, 1),  // Corrupted Lodestone x1
            (24327, 24326, RADIANT, 8, 4),      // Crystal Fragment x4
            (24328, 24327, LUMINOUS, 6, 3),     // Crystal Shard x3
            (24329, 24328, INCANDESCENT, 4, 2), // Crystal Core x2
            (24330, 24329, CRYSTALLINE, 2, 1),  // Crystal Lodestone x1
            (24322, 24321, RADIANT, 8, 4),      // Destroyer Fragment x4
            (24323, 24322, LUMINOUS, 6, 3),     // Destroyer Shard x3
            (24324, 24323, INCANDESCENT, 4, 2), // Destroyer Core x2
            (24325, 24324, CRYSTALLINE, 2, 1),  // Destroyer Lodestone x1
            (24317, 24316, RADIANT, 8, 4),      // Glacial Fragment x4
            (24318, 24317, LUMINOUS, 6, 3),     // Glacial Shard x3
            (24319, 24318, INCANDESCENT, 4, 2), // Glacial Core x2
            (24320, 24319, CRYSTALLINE, 2, 1),  // Glacial Lodestone x1
            (24312, 24311, RADIANT, 8, 4),      // Molten Fragment x4
            (24313, 24312, LUMINOUS, 6, 3),     // Molten Shard x3
            (24314, 24313, INCANDESCENT, 4, 2), // Molten Core x2
            (24315, 24314, CRYSTALLINE, 2, 1),  // Molten Lodestone x1
            (24307, 24306, RADIANT, 8, 4),      // Onyx Fragment x4
            (24308, 24307, LUMINOUS, 6, 3),     // Onyx Shard x3
            (24309, 24308, INCANDESCENT, 4, 2), // Onyx Core x2
            (24310, 24309, CRYSTALLINE, 2, 1),  // Onyx Lodestone x1
        ];
        steps
            .iter()
            .map(|&(output, base, dust, agent_n, agent_out)| Recipe {
                id: None,
                output_item_id: output,
                output_item_count: agent_out,
                disciplines: vec![config::Discipline::MysticForge],
                ingredients: vec![
                    api::RecipeIngredient {
                        item_id: base,
                        count: agent_n,
                    },
                    api::RecipeIngredient {
                        item_id: AGENT,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: dust,
                        count: 1,
                    },
                    api::RecipeIngredient {
                        item_id: CRYSTAL,
                        count: 1,
                    },
                ],
                source: RecipeSource::Automatic,
            })
            .collect()
    }
}

pub fn mark_recursive_recipes(recipes_map: &HashMap<u32, Recipe>) -> HashSet<u32> {
    let mut set = HashSet::new();
    for (recipe_id, recipe) in recipes_map {
        mark_recursive_recipes_internal(
            *recipe_id,
            recipe.output_item_id,
            recipes_map,
            &mut vec![],
            &mut set,
        );
    }
    set
}

fn mark_recursive_recipes_internal(
    item_id: u32,
    search_output_item_id: u32,
    recipes_map: &HashMap<u32, Recipe>,
    ingredients_stack: &mut Vec<u32>,
    set: &mut HashSet<u32>,
) {
    if set.contains(&item_id) {
        return;
    }
    if let Some(recipe) = recipes_map.get(&item_id) {
        for ingredient in &recipe.ingredients {
            if ingredient.item_id == search_output_item_id {
                set.insert(recipe.output_item_id);
                return;
            }
            // skip unnecessary recursion
            if ingredients_stack.contains(&ingredient.item_id) {
                continue;
            }
            ingredients_stack.push(ingredient.item_id);
            mark_recursive_recipes_internal(
                ingredient.item_id,
                search_output_item_id,
                recipes_map,
                ingredients_stack,
                set,
            );
            ingredients_stack.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn mystic_forge_recipes_shape() {
        let recipes = Recipe::mystic_forge_recipes();
        // 7 families x (fragment, shard, core, lodestone) x (wine excluded)
        assert_eq!(recipes.len(), 28);
        let mut outputs = HashSet::new();
        for recipe in &recipes {
            // all forge rows: no unlock tracking, MysticForge discipline
            assert_eq!(recipe.id, None);
            assert_eq!(recipe.disciplines, vec![config::Discipline::MysticForge]);
            assert!(recipe.is_automatic());
            // fixed agent-variant quantities only
            assert!([4, 3, 2, 1].contains(&recipe.output_item_count));
            assert_eq!(recipe.ingredients.len(), 4);
            // binding agent + crystal legs on every row
            let ids: Vec<u32> = recipe.ingredients.iter().map(|i| i.item_id).collect();
            assert!(ids.contains(&39125)); // Mystic Binding Agent
            assert!(ids.contains(&20799)); // Mystic Crystal
            assert!(outputs.insert(recipe.output_item_id));
        }
        // spot-check: Glacial Shard x3 from 6 fragments
        let shard = recipes
            .iter()
            .find(|r| r.output_item_id == 24318)
            .expect("Glacial Shard promotion");
        assert_eq!(shard.output_item_count, 3);
        assert!(shard
            .ingredients
            .iter()
            .any(|i| i.item_id == 24317 && i.count == 6));
    }
}
