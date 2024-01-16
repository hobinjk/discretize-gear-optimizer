use crate::{
    data::{
        affix::Affix,
        attribute::Attribute,
        character::{AttributesArray, Character},
        combination::Combination,
        settings::{Condition, Settings},
        BENCHMARK_ITERATIONS_PER_SETTING, PROGRESS_UPDATE_INTERVALL,
    },
    result::Result,
    utils::{clamp, get_random_affix_combination, get_total_combinations, round_even},
};
use std::{cell::RefCell, collections::HashMap};
use wasm_bindgen::JsValue;
use web_sys::{console, DedicatedWorkerGlobalScope};

/// Starts the optimization process. Calculates all possible combinations for the given chunk (subtree) of the affix tree.
/// This process is independent of the other chunks.
///
/// # Arguments
/// * `chunks` - A vector of vectors of affixes. Each chunk represents a subtree of the affix tree. The chunks are generated by the JS code and distributed to multiple web workers.
/// * `settings` - The settings. Contains important optimizer settings.
/// * `combinations` - A vector of extras combinations. To calculate the best runes and sigils we must calculate the resulting stats for each combination of extras. Also contains important optimizer settings.
/// * `workerglobal` - The web worker global scope. Used to post messages to the JS code.
pub fn start(
    chunks: &Vec<Vec<Affix>>,
    settings: &Settings,
    combinations: &Vec<Combination>,
    workerglobal: Option<&DedicatedWorkerGlobalScope>,
) -> Result {
    let rankby = settings.rankby;
    // calculate the number of results we need to store;
    let result_num = settings.maxResults; // as f32 / total_threads as f32;
    let total_combinations = get_total_combinations(settings, combinations.len());

    // we store our results in a Result object
    let mut result: Result = Result::new(result_num as usize);

    let counter = RefCell::new(0);
    let mut character = Character::new(rankby);

    let max_depth = settings.slots;

    // this callback is called for every affix combination (leaf). this is where we calculate the resulting stats
    // crucuial to optimize every call in this function as it will be called millions of times
    let mut callback = |subtree: &[Affix]| {
        // Leaf callback implementation

        // iterate over all combinations
        for i in 0..combinations.len() {
            let combination = &combinations[i];
            character.clear();
            character.combination_id = i as u32;

            // calculate stats for this combination
            let valid = test_character(&mut character, settings, combination, subtree);

            if valid {
                // insert into result_characters if better than worst character
                result.insert(&character);
            }
            *counter.borrow_mut() += 1;

            // post message to js
            if *counter.borrow() % PROGRESS_UPDATE_INTERVALL == 0 {
                result.on_complete(settings, combinations);

                // get json value of best characters
                let mut best_combinations: Vec<Combination> = vec![];
                let mut combination_indices: HashMap<u32, usize> = HashMap::new();
                let mut best_characters = result.best_characters.clone();

                best_characters.iter_mut().for_each(|character| {
                    let combination = combinations.get(character.combination_id as usize).unwrap();
                    let current_id = character.combination_id;
                    if let Some(comb_index) = combination_indices.get(&current_id) {
                        character.combination_id = *comb_index as u32;
                    } else {
                        let comb_index = best_combinations.len();
                        combination_indices.insert(current_id, comb_index);
                        best_combinations.push(combination.clone());
                        character.combination_id = comb_index as u32;
                    }
                });
                let best_character_json = serde_json::to_string(&best_characters).unwrap();
                let best_combinations_json = serde_json::to_string(&best_combinations).unwrap();

                workerglobal.and_then(|w| {
                    w.post_message(&JsValue::from_str(&format!(
                        "{{ \"type\": \"PROGRESS\", \"total\": {}, \"new\": {}, \"results\": {}, \"combinations\": {} }}",
                        total_combinations, PROGRESS_UPDATE_INTERVALL,best_character_json, best_combinations_json
                    )))
                    .ok()
                });
            }
        }
    };

    for chunk in chunks {
        // start dfs into tree
        descend_subtree_dfs(
            &settings.affixesArray,
            chunk,
            max_depth as usize,
            &mut callback,
        );
    }

    result
}

/// Runs a couple benchmarking runs first to get a good heuristics which combinations to test first.
/// Each combination is tested BENCHMARK_ITERATIONS_PER_SETTING times with random affix combinations.
/// We figure out how often each setting in the resulting character appears in the top BENCMARK_ITERATIONS_PER_SETTING characters.
/// Based on this we pick the settings that have at least a 10% likelyhood of being "good" and start the optimization process with those.
/// We then run the usual process with the picked settings.
///
/// # Arguments
/// * `chunks` - A vector of vectors of affixes. Each chunk represents a subtree of the affix tree. The chunks are generated by the JS code and distributed to multiple web workers.
/// * `settings` - The settings. Contains important optimizer settings.
/// * `combinations` - A vector of extras combinations. To calculate the best runes and sigils we must calculate the resulting stats for each combination of extras. Also contains important optimizer settings.
/// * `workerglobal` - The web worker global scope. Used to post messages to the JS code.
pub fn start_with_heuristics(settings: &Settings, combinations: &Vec<Combination>) -> Vec<u32> {
    let mut result = Result::new(BENCHMARK_ITERATIONS_PER_SETTING as usize);

    // benchmark a few results first to get a good heuristics which combinations to test first
    let mut character = Character::new(settings.rankby);

    for (index, combination) in combinations.iter().enumerate() {
        for _ in 0..BENCHMARK_ITERATIONS_PER_SETTING {
            character.clear();
            character.combination_id = index as u32;

            let gear =
                get_random_affix_combination(&settings.affixesArray, settings.slots as usize);

            // calculate stats for this combination
            let valid = test_character(&mut character, settings, combination, &gear);
            if valid {
                // insert into result_characters if better than worst character
                result.insert(&character);
            }
        }
    }

    // count occurences of settings in result
    let weighted_combinations = result.get_weighted_combinations(combinations);

    let picked_combinations: Vec<u32> = weighted_combinations
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if *value > 10 {
                Some(index as u32)
            } else {
                None
            }
        })
        .collect();

    console::log_1(&JsValue::from_str(&format!(
        "Finished heuristics. Picked {} combinations",
        picked_combinations.len()
    )));

    picked_combinations
}

/// Uses depth-first search to calculate all possible combinations of affixes for the given subtree.
///
/// # Arguments
/// * `affix_array` - An array of vectors of affixes. Each entry in the array corresponds to the affixes selectable for a specific slot. The array is of length 14, because there are 14 slots. However, if the last slot is not used due to two-handed weapons, the last entry in the array is Affix::None
/// * `subtree` - The current subtree of the affix tree. This is a vector of affixes. The length of the vector is the current layer of the tree. The first entry in the vector is the root of the tree.
/// * `leaf_callback` - A function that is called when a leaf of the tree is reached. The function is passed the current subtree.
pub fn descend_subtree_dfs<F>(
    affix_array: &[Vec<Affix>],
    subtree: &[Affix],
    max_depth: usize,
    leaf_callback: &mut F,
) where
    F: FnMut(&[Affix]),
{
    let current_layer = subtree.len();

    if current_layer == max_depth {
        // if we reached leafs of the tree, call the function
        leaf_callback(subtree);
    } else {
        let permutation_options = &affix_array[current_layer];

        let mut new_subtree: Vec<Affix> = Vec::with_capacity(subtree.len() + 1);
        new_subtree.clear();
        new_subtree.extend_from_slice(subtree);

        for &option in permutation_options {
            new_subtree.push(option);
            descend_subtree_dfs(affix_array, &new_subtree, max_depth, leaf_callback);
            new_subtree.pop();
        }
    }
}

pub fn test_character(
    character: &mut Character,
    settings: &Settings,
    combination: &Combination,
    subtree: &[Affix],
) -> bool {
    // add base attributes from settings to character
    combination.baseAttributes.iter().for_each(|(key, value)| {
        character.base_attributes.set_a(*key, *value);
    });

    for (index, affix) in subtree.iter().enumerate() {
        // find out stats for each affix and add them to the character
        let index_in_affix_array = settings.affixesArray[index]
            .iter()
            .position(|&r| r.to_number() == affix.to_number());
        if index_in_affix_array.is_none() {
            println!(
                "Affix not found in affixesArray {} {}. Is your input valid?",
                index,
                affix.to_number()
            );
            break;
        }

        let attributes_to_add = &settings.affixStatsArray[index][index_in_affix_array.unwrap()];

        attributes_to_add.iter().for_each(|(key, value)| {
            character.base_attributes.add_a(*key, *value);
        });

        character.gear[index] = *affix;
    }

    // calculate stats for the character
    update_attributes(character, settings, combination, false)
}

pub fn update_attributes(
    character: &mut Character,
    settings: &Settings,
    combination: &Combination,
    no_rounding: bool,
) -> bool {
    calc_stats(character, settings, combination, no_rounding);

    if character.is_invalid(settings) {
        return false;
    }

    let power_damage_score = calc_power(character, settings, combination);
    let condi_damage_score = calc_condi(
        character,
        settings,
        combination,
        &combination.relevantConditions,
    );

    character.attributes.set_a(
        Attribute::Damage,
        power_damage_score + condi_damage_score + character.attributes.get_a(Attribute::FlatDPS),
    );

    calc_survivability(character, combination);
    calc_healing(character);

    true
}

fn calc_stats(
    character: &mut Character,
    settings: &Settings,
    combination: &Combination,
    no_rounding: bool,
) {
    // move base attributes to attributes as default
    // not sure which method is faster, but I think the for loop is faster:
    // 1. for loop
    // 2. clone
    for i in 0..character.base_attributes.len() {
        character.attributes[i] = character.base_attributes[i]
    }
    //character.attributes = character.base_attributes.clone();

    // get references to play with
    let attributes = &mut character.attributes;
    let base_attributes = &character.base_attributes;

    // closure for rounding values depending on no_rounding
    let round = |val: f32| {
        if no_rounding {
            val
        } else {
            round_even(val)
        }
    };

    // handle convert modifiers
    for (attribute, conversion) in &combination.modifiers.convert {
        let maybe_round = |val: f32| {
            if attribute.is_point_key() {
                round(val)
            } else {
                val
            }
        };

        for (source, percent) in conversion {
            attributes.add_a(
                *attribute,
                maybe_round(base_attributes.get_a(*source) * percent),
            );
        }
    }

    // handle buff modifiers, these are simply added to the existing attributes
    for (attribute, bonus) in &combination.modifiers.buff {
        attributes.add_a(*attribute, *bonus);
    }

    // handle convertAfterBuffs modifiers
    for (attribute, conversion) in &combination.modifiers.convertAfterBuffs {
        let maybe_round = |val: f32| {
            if attribute.is_point_key() {
                round(val)
            } else {
                val
            }
        };

        for (source, percent) in conversion {
            match *source {
                Attribute::CriticalChance => {
                    attributes.add_a(
                        *attribute,
                        maybe_round(
                            clamp(attributes.get_a(Attribute::CriticalChance), 0.0, 1.0) * percent,
                        ),
                    );
                }
                Attribute::CloneCriticalChance => {
                    // replace macro with set
                    attributes.add_a(
                        *attribute,
                        maybe_round(
                            clamp(attributes.get_a(Attribute::CloneCriticalChance), 0.0, 1.0)
                                * percent,
                        ),
                    );
                }
                Attribute::PhantasmCriticalChance => {
                    attributes.add_a(
                        *attribute,
                        maybe_round(
                            clamp(
                                attributes.get_a(Attribute::PhantasmCriticalChance),
                                0.0,
                                1.0,
                            ) * percent,
                        ),
                    );
                }

                _ => {
                    attributes.add_a(*attribute, maybe_round(attributes.get_a(*source) * percent));
                }
            }
        }
    }

    // recalculate attributes
    attributes.add_a(
        Attribute::CriticalChance,
        (attributes.get_a(Attribute::Precision) - 1000.0) / 21.0 / 100.0,
    );
    attributes.add_a(
        Attribute::CriticalDamage,
        attributes.get_a(Attribute::Ferocity) / 15.0 / 100.0,
    );
    attributes.add_a(
        Attribute::BoonDuration,
        attributes.get_a(Attribute::Concentration) / 15.0 / 100.0,
    );
    attributes.set_a(
        Attribute::Health,
        round(
            (attributes.get_a(Attribute::Health) + attributes.get_a(Attribute::Vitality) * 10.0)
                * (1.0 + attributes.get_a(Attribute::MaxHealth)),
        ),
    );

    // clones/phantasms/shroud
    if settings.profession.eq("Mesmer") {
        attributes.add_a(
            Attribute::CloneCriticalChance,
            (attributes.get_a(Attribute::Precision) - 1000.0) / 21.0 / 100.0,
        );
        attributes.add_a(
            Attribute::PhantasmCriticalChance,
            (attributes.get_a(Attribute::Precision) - 1000.0) / 21.0 / 100.0,
        );
        attributes.add_a(
            Attribute::PhantasmCriticalDamage,
            attributes.get_a(Attribute::Ferocity) / 15.0 / 100.0,
        );
    } else if attributes.get_a(Attribute::Power2Coefficient) > 0.0 {
        attributes.set_a(
            Attribute::AltPower,
            attributes.get_a(Attribute::AltPower) + attributes.get_a(Attribute::Power),
        );
        attributes.set_a(
            Attribute::AltCriticalChance,
            attributes.get_a(Attribute::AltCriticalChance)
                + attributes.get_a(Attribute::CriticalChance)
                + attributes.get_a(Attribute::AltPrecision) / 21.0 / 100.0,
        );
        attributes.set_a(
            Attribute::AltCriticalDamage,
            attributes.get_a(Attribute::AltCriticalDamage)
                + attributes.get_a(Attribute::CriticalDamage)
                + attributes.get_a(Attribute::AltFerocity) / 15.0 / 100.0,
        );
    }
}

pub fn calc_power(
    character: &mut Character,
    settings: &Settings,
    combination: &Combination,
) -> f32 {
    let attributes = &mut character.attributes;
    let mods = &combination.modifiers;

    let crit_dmg = attributes.get_a(Attribute::CriticalDamage)
        * mods.get_dmg_multiplier(Attribute::OutgoingCriticalDamage);
    let crit_chance = clamp(attributes.get_a(Attribute::CriticalChance), 0.0, 1.0);

    attributes.set_a(
        Attribute::EffectivePower,
        attributes.get_a(Attribute::Power)
            * (1.0 + crit_chance * (crit_dmg - 1.0))
            * mods.get_dmg_multiplier(Attribute::OutgoingStrikeDamage),
    );
    attributes.set_a(
        Attribute::NonCritEffectivePower,
        attributes.get_a(Attribute::Power)
            * mods.get_dmg_multiplier(Attribute::OutgoingStrikeDamage),
    );

    // 2597: standard enemy armor value, also used for ingame damage tooltips
    let mut power_damage = (attributes.get_a(Attribute::PowerCoefficient) / 2597.0)
        * attributes.get_a(Attribute::EffectivePower)
        + (attributes.get_a(Attribute::NonCritPowerCoefficient) / 2597.0)
            * attributes.get_a(Attribute::NonCritEffectivePower);
    // this is nowhere read again?
    attributes.set_a(Attribute::PowerDPS, power_damage);

    if attributes.get_a(Attribute::Power2Coefficient) > 0.0 {
        // do stuff
        if settings.profession.eq("Mesmer") {
            let phantasm_crit_dmg = attributes.get_a(Attribute::PhantasmCriticalDamage)
                * mods.get_dmg_multiplier(Attribute::OutgoingPhantasmCriticalDamage);
            let phantasm_crit_chance = clamp(
                attributes.get_a(Attribute::PhantasmCriticalChance),
                0.0,
                1.0,
            );

            attributes.set_a(
                Attribute::PhantasmEffectivePower,
                attributes.get_a(Attribute::Power)
                    * (1.0 + phantasm_crit_chance * (phantasm_crit_dmg - 1.0))
                    * mods.get_dmg_multiplier(Attribute::OutgoingPhantasmDamage),
            );

            let phantasm_power_damage = (attributes.get_a(Attribute::Power2Coefficient) / 2597.0)
                * attributes.get_a(Attribute::PhantasmEffectivePower);
            attributes.set_a(Attribute::Power2DPS, phantasm_power_damage);
            power_damage += phantasm_power_damage;
        } else {
            let alt_crit_dmg = attributes.get_a(Attribute::AltCriticalDamage)
                * mods.get_dmg_multiplier(Attribute::OutgoingAltCriticalDamage);
            let alt_crit_chance = clamp(attributes.get_a(Attribute::AltCriticalChance), 0.0, 1.0);

            attributes.set_a(
                Attribute::AltEffectivePower,
                attributes.get_a(Attribute::AltPower)
                    * (1.0 + alt_crit_chance * (alt_crit_dmg - 1.0))
                    * mods.get_dmg_multiplier(Attribute::OutgoingStrikeDamage)
                    * mods.get_dmg_multiplier(Attribute::OutgoingAltDamage),
            );

            let alt_power_damage = (attributes.get_a(Attribute::Power2Coefficient) / 2597.0)
                * attributes.get_a(Attribute::AltEffectivePower);
            attributes.set_a(Attribute::Power2DPS, alt_power_damage);
            power_damage += alt_power_damage;
        }
    } else {
        attributes.set_a(Attribute::Power2DPS, 0.0);
    }

    let siphon_damage = attributes.get_a(Attribute::SiphonBaseCoefficient)
        * mods.get_dmg_multiplier(Attribute::OutgoingSiphonDamage);

    attributes.set_a(Attribute::SiphonDPS, siphon_damage);

    power_damage + siphon_damage
}

/// Calculates a damage tick for a given condition
///
/// # Arguments
/// - `condition` - the condition to calculate the damage for
/// - `cdmg` - the condition damage of the character
/// - `mult` - the damage multiplier
/// - `wvw` - whether the calculation is for wvw or not
/// - `special` - whether the calculation is for a special condition or not such as ConfusionActive or TormentMoving
fn condition_damage_tick(
    condition: &Condition,
    cdmg: f32,
    mult: f32,
    wvw: bool,
    special: bool,
) -> f32 {
    (condition.get_factor(wvw, special) * cdmg + condition.get_base_damage(wvw, special)) * mult
}

pub fn calc_condi(
    character: &mut Character,
    settings: &Settings,
    combination: &Combination,
    relevant_conditions: &[Condition],
) -> f32 {
    let attributes = &mut character.attributes;
    let mods = &combination.modifiers;

    attributes.add_a(
        Attribute::ConditionDuration,
        attributes.get_a(Attribute::Expertise) / 15.0 / 100.0,
    );

    let mut condi_damage_score = 0.0;
    // iterate over all (relevant) conditions
    for condition in relevant_conditions.iter() {
        let cdmg = attributes.get_a(Attribute::ConditionDamage);
        let mult = mods.get_dmg_multiplier(Attribute::OutgoingConditionDamage)
            * mods.get_dmg_multiplier(condition.get_damage_mod_attribute());

        match condition {
            Condition::Confusion => {
                attributes.set_a(
                    Attribute::ConfusionDamageTick,
                    condition_damage_tick(condition, cdmg, mult, settings.is_wvw(), false)
                        + condition_damage_tick(condition, cdmg, mult, settings.is_wvw(), true)
                            * settings.attackRate,
                );
            }
            Condition::Torment => {
                attributes.set_a(
                    Attribute::TormentDamageTick,
                    condition_damage_tick(condition, cdmg, mult, settings.is_wvw(), false)
                        * (1.0 - settings.movementUptime)
                        + condition_damage_tick(condition, cdmg, mult, settings.is_wvw(), true)
                            * settings.movementUptime,
                );
            }
            _ => attributes.set_a(
                condition.get_damage_tick_attribute(),
                condition_damage_tick(condition, cdmg, mult, settings.is_wvw(), false),
            ),
        }

        let coeff = attributes.get_a(condition.get_coefficient_attribute());

        let duration = 1.0
            + clamp(
                attributes.get_a(condition.get_duration_attribute())
                    + attributes.get_a(Attribute::ConditionDuration),
                0.0,
                1.0,
            );

        let stacks = coeff * duration;
        attributes.set_a(condition.get_stacks_attribute(), stacks);

        let tick_attr = attributes.get_a(condition.get_damage_tick_attribute());
        let dps = stacks * if tick_attr > 0.0 { tick_attr } else { 1.0 };
        attributes.set_a(condition.get_dps_attribute(), dps);

        condi_damage_score += dps;
    }

    condi_damage_score
}

fn calc_survivability(character: &mut Character, combination: &Combination) {
    let attributes = &mut character.attributes;
    let mods = &combination.modifiers;

    attributes.add_a(Attribute::Armor, attributes.get_a(Attribute::Toughness));

    attributes.set_a(
        Attribute::EffectiveHealth,
        attributes.get_a(Attribute::Health)
            * attributes.get_a(Attribute::Armor)
            * (1.0 / mods.get_dmg_multiplier(Attribute::IncomingStrikeDamage)),
    );

    attributes.set_a(
        Attribute::Survivability,
        attributes.get_a(Attribute::EffectiveHealth) / 1967.0,
    );
}

fn calc_healing(character: &mut Character) {
    let attributes = &mut character.attributes;

    // reasonably representative skill: druid celestial avatar 4 pulse
    // 390 base, 0.3 coefficient
    attributes.set_a(
        Attribute::EffectiveHealing,
        (attributes.get_a(Attribute::HealingPower) * 0.3 + 390.0)
            * (1.0 + attributes.get_a(Attribute::OutgoingHealing)),
    );

    // TODO add bountiful maintenance oil

    attributes.set_a(
        Attribute::Healing,
        attributes.get_a(Attribute::EffectiveHealing),
    );
}
