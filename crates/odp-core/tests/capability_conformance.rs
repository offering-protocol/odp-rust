//! Filter and Sort Definitions, the search requests that reference them, and the Refinement
//! Groups a search response answers with.

mod support;

use odp_core::{
    FilterOperator, FilterType, ParseError, SortDirection, parse_agent_offering_search_response,
    parse_collection_search_request, parse_filter_definition, parse_filter_definition_page,
    parse_offering_search_request, parse_offering_search_response, parse_page,
    parse_sort_definition, parse_sort_definition_page,
};
use serde_json::{Value, json};
use support::{amend, assert_rejected_for, encode, filter_definition, page, sort_definition};

fn read_filter(changes: Value) -> Result<odp_core::FilterDefinition, ParseError> {
    parse_filter_definition(&encode(&amend(filter_definition("integer"), changes)))
}

fn kilograms() -> Value {
    json!({"system": "ucum", "code": "kg"})
}

#[test]
fn reads_a_conformant_filter_definition() {
    let definition = read_filter(json!({"operators": ["eq", "gte"], "refinable": true})).unwrap();

    assert_eq!(definition.id, "weight");
    assert_eq!(definition.filter_type, FilterType::Integer);
    assert_eq!(definition.operators[1], FilterOperator::GreaterThanOrEqual);
    assert!(definition.refinable);
}

// -- units -----------------------------------------------------------------------------------------

/// FLT-10: only a numeric Filter carries a unit, because only a numeric Filter has a dimension.
#[test]
fn refuses_a_unit_on_a_filter_that_measures_nothing() {
    for filter_type in ["boolean", "date", "date-time", "string"] {
        assert_rejected_for(
            parse_filter_definition(&encode(&amend(
                filter_definition(filter_type),
                json!({"unit": kilograms()}),
            ))),
            "unit-type",
        );
    }
}

#[test]
fn accepts_a_unit_on_every_numeric_filter() {
    for filter_type in ["decimal", "integer", "number"] {
        assert!(
            parse_filter_definition(&encode(&amend(
                filter_definition(filter_type),
                json!({"unit": kilograms()}),
            )))
            .is_ok(),
            "{filter_type}"
        );
    }
}

/// A non-numeric Filter that claims no unit is exactly what the rule asks for.
#[test]
fn accepts_every_filter_type_without_a_unit() {
    for filter_type in [
        "boolean",
        "date",
        "date-time",
        "decimal",
        "integer",
        "number",
        "string",
    ] {
        assert!(
            parse_filter_definition(&encode(&filter_definition(filter_type))).is_ok(),
            "{filter_type}"
        );
    }
}

/// FLT-14: a unit is defined inline, under a system ODP names.
#[test]
fn refuses_a_unit_from_a_system_odp_does_not_name() {
    assert!(read_filter(json!({"unit": {"system": "imperial", "code": "lb"}})).is_err());
    assert!(
        read_filter(json!({
            "unit": {"system": "service", "code": "pot-size", "title": "Pot size"}
        }))
        .is_ok()
    );
}

// -- operators -------------------------------------------------------------------------------------

/// FLT-11: an ordering operator on a type that has no order cannot be evaluated.
#[test]
fn refuses_an_ordering_operator_on_an_unordered_type() {
    for filter_type in ["boolean", "string"] {
        for operator in ["gt", "gte", "lt", "lte"] {
            assert_rejected_for(
                parse_filter_definition(&encode(&amend(
                    filter_definition(filter_type),
                    json!({"operators": ["eq", operator]}),
                ))),
                "operator-type",
            );
        }
    }
}

/// Dates and numbers are ordered, so the same operators are fine there.
#[test]
fn accepts_an_ordering_operator_on_an_ordered_type() {
    for filter_type in ["date", "date-time", "decimal", "integer", "number"] {
        assert!(
            parse_filter_definition(&encode(&amend(
                filter_definition(filter_type),
                json!({"operators": ["gt", "lte"]}),
            )))
            .is_ok(),
            "{filter_type}"
        );
    }
}

/// FLT-12: a refinable definition counts values, which needs an equality operator to count by.
#[test]
fn refuses_a_refinable_definition_that_cannot_compare_values() {
    assert!(read_filter(json!({"operators": ["exists"], "refinable": true})).is_err());
    assert!(read_filter(json!({"operators": ["eq"], "refinable": true})).is_ok());
    assert!(read_filter(json!({"operators": ["in"], "refinable": true})).is_ok());
}

/// FLT-09: an operator ODP does not define is not one a Service can advertise.
#[test]
fn refuses_an_operator_odp_does_not_define() {
    assert!(read_filter(json!({"operators": ["startswith"]})).is_err());
    assert!(read_filter(json!({"operators": []})).is_err());
}

// -- Sort Definitions --------------------------------------------------------------------------------

#[test]
fn reads_a_conformant_sort_definition() {
    let definition = parse_sort_definition(&encode(&sort_definition())).unwrap();

    assert_eq!(definition.id, "cheapest");
    assert_eq!(definition.keys[0].direction, SortDirection::Ascending);
}

/// SRT-05: a recipe orders by between one and three keys.
#[test]
fn refuses_a_recipe_with_no_keys_or_too_many() {
    let key =
        |filter_id| json!({"filter_id": filter_id, "direction": "ascending", "missing": "last"});

    assert!(
        parse_sort_definition(&encode(&amend(sort_definition(), json!({"keys": []})))).is_err()
    );
    assert!(
        parse_sort_definition(&encode(&amend(
            sort_definition(),
            json!({"keys": [key("a"), key("b"), key("c"), key("d")]}),
        )))
        .is_err()
    );
}

/// SRT-07: a key orders in a direction ODP names, and says where missing values go.
#[test]
fn refuses_a_key_that_orders_in_no_direction_odp_names() {
    for key in [
        json!({"filter_id": "price", "direction": "sideways", "missing": "last"}),
        json!({"filter_id": "price", "direction": "ascending", "missing": "middle"}),
        json!({"filter_id": "price", "direction": "ascending"}),
    ] {
        assert!(
            parse_sort_definition(&encode(&amend(sort_definition(), json!({"keys": [key]}))))
                .is_err()
        );
    }
}

// -- capability pages ----------------------------------------------------------------------------------

#[test]
fn reads_a_page_of_each_kind_of_definition() {
    let filters =
        parse_filter_definition_page(&encode(&page(json!([filter_definition("string")])))).unwrap();
    assert_eq!(filters.items[0].id, "weight");
    assert!(filters.next.is_empty());

    let sorts = parse_sort_definition_page(&encode(&page(json!([sort_definition()])))).unwrap();
    assert_eq!(sorts.items[0].keys.len(), 1);
}

/// PAG-04: a page carries at most 100 items, whatever the caller asked for.
#[test]
fn refuses_a_page_beyond_the_item_limit() {
    let items: Vec<Value> = (0..101)
        .map(|index| {
            amend(
                filter_definition("string"),
                json!({"id": format!("f{index}")}),
            )
        })
        .collect();

    assert!(parse_filter_definition_page(&encode(&page(json!(items)))).is_err());
}

/// PAG-06: a continuation is a reference the Agent can follow back to the same Service.
#[test]
fn reads_a_page_that_offers_a_continuation() {
    let value = amend(
        page(json!([])),
        json!({"next": "/odp/offerings?cursor=2", "auth_expands": true}),
    );
    let read = parse_page::<Value>(&encode(&value)).unwrap();

    assert_eq!(read.next, "/odp/offerings?cursor=2");
    assert!(read.auth_expands);
}

/// `auth_expands` says the signed-in view is wider; saying it is not is saying nothing.
#[test]
fn refuses_a_page_that_denies_expansion_rather_than_omitting_it() {
    let value = amend(page(json!([])), json!({"auth_expands": false}));
    assert!(parse_page::<Value>(&encode(&value)).is_err());
}

// -- search requests -------------------------------------------------------------------------------------

#[test]
fn reads_a_conformant_offering_search_request() {
    let request = parse_offering_search_request(
        br#"{"odp_version":"1.0","query":"monstera","limit":20,"collection_id":"plants","include_descendants":true,"sort":"cheapest","refinements":["colour"],"filters":[{"id":"weight","operator":"gte","value":2}]}"#,
    )
    .unwrap();

    assert_eq!(request.query, "monstera");
    assert_eq!(request.limit, 20);
    assert_eq!(request.refinements, ["colour"]);
    assert_eq!(
        request.filters[0].operator,
        FilterOperator::GreaterThanOrEqual
    );
}

#[test]
fn reads_a_conformant_collection_search_request() {
    let request = parse_collection_search_request(
        br#"{"odp_version":"1.0","query":"plants","parent_id":"garden","limit":5}"#,
    )
    .unwrap();

    assert_eq!(request.parent_id, Some(Some("garden".to_owned())));
    assert_eq!(request.limit, 5);
}

/// COL-31: a null parent asks for the Collections at the root, which is not the same question as
/// asking with no parent at all.
#[test]
fn tells_a_null_parent_apart_from_an_absent_one() {
    let rooted =
        parse_collection_search_request(br#"{"odp_version":"1.0","parent_id":null}"#).unwrap();
    let anywhere =
        parse_collection_search_request(br#"{"odp_version":"1.0","query":"plants"}"#).unwrap();

    assert_eq!(rooted.parent_id, Some(None));
    assert_eq!(anywhere.parent_id, None);
}

/// A Collection search asks by query or by parent; asking neither asks nothing.
#[test]
fn refuses_a_collection_search_that_asks_nothing() {
    assert!(parse_collection_search_request(br#"{"odp_version":"1.0"}"#).is_err());
}

/// FLT-25: a Filter Expression names its definition, an operator, and a value to compare against.
#[test]
fn refuses_a_filter_expression_that_compares_nothing() {
    for filters in [
        json!([{"id": "weight", "operator": "gte"}]),
        json!([{"id": "weight", "value": 2}]),
        json!([{"operator": "gte", "value": 2}]),
        json!([{"id": "weight", "operator": "gte", "value": []}]),
        json!([{"id": "weight", "operator": "gte", "value": {"amount": 2}}]),
    ] {
        let request = json!({"odp_version": "1.0", "filters": filters});
        assert!(
            parse_offering_search_request(&encode(&request)).is_err(),
            "{filters}"
        );
    }
}

/// PAG-03: a request cannot ask for more than a page holds.
#[test]
fn refuses_a_limit_beyond_what_a_page_holds() {
    let request = json!({"odp_version": "1.0", "limit": 101});
    assert!(parse_offering_search_request(&encode(&request)).is_err());
}

// -- refinements ---------------------------------------------------------------------------------------------

fn search_response(refinements: Value) -> Vec<u8> {
    encode(&json!({"odp_version": "1.0", "items": [], "refinements": refinements}))
}

fn group(filter_id: &str, values: Value) -> Value {
    json!({"filter_id": filter_id, "values": values})
}

#[test]
fn reads_a_conformant_search_response() {
    let page = parse_offering_search_response(&search_response(json!([group(
        "colour",
        json!([{"value": "green", "count": 4}, {"value": "red", "count": 2, "count_relation": "lower_bound"}])
    )])))
    .unwrap();

    assert_eq!(page.refinements[0].filter_id, "colour");
    assert_eq!(page.refinements[0].values[0].count, 4);
    assert_eq!(page.refinements[0].values[1].count_relation, "lower_bound");
}

/// FLT-30: `filter_id` is unique among the returned groups, so a repeat leaves an Agent unable to
/// say which group belongs to that Filter Definition.
#[test]
fn refuses_two_groups_for_one_filter() {
    assert_rejected_for(
        parse_offering_search_response(&search_response(json!([
            group("colour", json!([{"value": "green", "count": 4}])),
            group("colour", json!([{"value": "red", "count": 2}]))
        ]))),
        "unique-filter-id",
    );
}

/// FLT-32: bucket values are unique within a group, so two counts never describe one candidate.
#[test]
fn refuses_a_repeated_bucket_value() {
    for values in [
        json!([{"value": "green", "count": 4}, {"value": "green", "count": 2}]),
        json!([{"value": true, "count": 4}, {"value": true, "count": 2}]),
        json!([{"value": 3, "count": 4}, {"value": 3.0, "count": 2}]),
    ] {
        assert_rejected_for(
            parse_offering_search_response(&search_response(json!([group("colour", values)]))),
            "unique-bucket-value",
        );
    }
}

/// FLT-32: decimal equality is numeric rather than lexical, so one value spelled two ways is one
/// value.
#[test]
fn reads_two_spellings_of_one_decimal_as_one_bucket_value() {
    for values in [
        json!([{"value": "1.0", "count": 4}, {"value": "1.00", "count": 2}]),
        json!([{"value": "0", "count": 4}, {"value": "0.0", "count": 2}]),
        json!([{"value": "12", "count": 4}, {"value": "12.000", "count": 2}]),
    ] {
        assert_rejected_for(
            parse_offering_search_response(&search_response(json!([group("weight", values)]))),
            "unique-bucket-value",
        );
    }
}

/// Two decimals that differ are still two values, and so is a string that only looks decimal.
#[test]
fn keeps_bucket_values_that_differ_apart() {
    for values in [
        json!([{"value": "1.0", "count": 4}, {"value": "2.0", "count": 2}]),
        json!([{"value": "1.01", "count": 4}, {"value": "1.1", "count": 2}]),
        json!([{"value": "10", "count": 4}, {"value": "1.0", "count": 2}]),
        // Neither is a decimal -- a leading zero and a trailing period are not ODP decimals -- so
        // both are compared as the strings they are.
        json!([{"value": "01", "count": 4}, {"value": "1", "count": 2}]),
        json!([{"value": "1.", "count": 4}, {"value": "1", "count": 2}]),
        json!([{"value": "-1.0", "count": 4}, {"value": "1.0", "count": 2}]),
        json!([{"value": true, "count": 4}, {"value": false, "count": 2}]),
        json!([{"value": 1, "count": 4}, {"value": "1", "count": 2}]),
    ] {
        assert!(
            parse_offering_search_response(&search_response(json!([group(
                "weight",
                values.clone()
            )])))
            .is_ok(),
            "{values}"
        );
    }
}

/// FLT-31: a response carries at most 16 groups, each of at most 32 buckets.
#[test]
fn refuses_more_groups_or_buckets_than_a_response_carries() {
    let groups: Vec<Value> = (0..17)
        .map(|index| group(&format!("f{index}"), json!([{"value": "a", "count": 1}])))
        .collect();
    assert!(parse_offering_search_response(&search_response(json!(groups))).is_err());

    let buckets: Vec<Value> = (0..33)
        .map(|index| json!({"value": index, "count": 1}))
        .collect();
    assert!(
        parse_offering_search_response(&search_response(json!([group("colour", json!(buckets))])))
            .is_err()
    );
}

/// FLT-33: a count is a non-negative integer, and `count_relation` says only that it is a floor.
#[test]
fn refuses_a_count_that_is_not_a_count() {
    for bucket in [
        json!({"value": "green", "count": -1}),
        json!({"value": "green", "count": 1.5}),
        json!({"value": "green"}),
        json!({"value": "green", "count": 1, "count_relation": "estimate"}),
        json!({"value": ["green"], "count": 1}),
    ] {
        assert!(
            parse_offering_search_response(&search_response(json!([group(
                "colour",
                json!([bucket])
            )])))
            .is_err(),
            "{bucket}"
        );
    }
}

/// ROLE-03: a Refinement Group an Agent cannot use is no reason to discard the Offering results
/// beside it, so the Agent entry point reads the page and leaves the group to report.
#[test]
fn hands_an_agent_a_page_a_service_must_not_publish() {
    let data = search_response(json!([
        group("colour", json!([{"value": "green", "count": 4}])),
        group("colour", json!([{"value": "red", "count": 2}]))
    ]));

    assert!(parse_offering_search_response(&data).is_err());
    assert_eq!(
        parse_agent_offering_search_response(&data)
            .unwrap()
            .refinements
            .len(),
        2
    );
}
