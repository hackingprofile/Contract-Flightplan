# Mapping language

Implemented operations. Unknown names are `unknown_mapping_operation` and are rejected before execution. Malformed parameters (wrong types, empty or duplicate names, unknown `format`) are rejected at decode.

## rename

Parameters: `{ "from": "<source>", "to": "<destination>" }`

Moves a top-level object field. Collision if `to` exists. Missing `from` fails. `from == to` is a no-op when the field exists. Type-check: `from` must exist in the source schema, `to` in the destination schema, and the destination type must accept the source type.

Example rule (from `packs/rename-user-id/proposal.json`; also `schemas/v1/examples/valid-rename.json`):

```json
{
  "id": "rename-user-request",
  "operation": "rename",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/users",
      "operation_id": "createUser"
    },
    "location": "request",
    "field": "id"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/users",
      "operation_id": "createUser"
    },
    "location": "request",
    "field": "user_id"
  },
  "parameters": {
    "from": "id",
    "to": "user_id"
  },
  "preconditions": [
    "source field id is present",
    "destination field user_id is absent"
  ],
  "postconditions": [
    "user_id holds the previous id value",
    "id is absent"
  ],
  "evidence": "POST /users request schema renamed id to user_id",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.collision.json",
    "request.missing.json",
    "request.malformed.json",
    "request.script.json"
  ]
}
```

## copy

Parameters: `{ "from": "<source>", "to": "<destination>" }`

Copies a field and leaves `from` in place. Collision and missing behave as rename.

Example rule (from `packs/copy-display-name/proposal.json`; also `schemas/v1/examples/valid-copy.json`):

```json
{
  "id": "copy-display-request",
  "operation": "copy",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/profiles",
      "operation_id": "createProfile"
    },
    "location": "request",
    "field": "name"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/profiles",
      "operation_id": "createProfile"
    },
    "location": "request",
    "field": "display_name"
  },
  "parameters": {
    "from": "name",
    "to": "display_name"
  },
  "preconditions": [
    "name is present",
    "display_name is absent"
  ],
  "postconditions": [
    "name remains",
    "display_name equals name"
  ],
  "evidence": "POST /profiles request copies name onto display_name",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.collision.json",
    "request.missing.json"
  ]
}
```

## default

Parameters: `{ "field": "<name>", "value": <json> }`

Inserts `value` when the key is absent. Explicit JSON `null` is `explicit_null` and is not replaced. Type-check: `value` must validate against the destination property schema. Lossiness `default_inserted` is recorded only when the insertion happened.

Example rule (from `packs/default-status/proposal.json`; also `schemas/v1/examples/valid-default.json`):

```json
{
  "id": "default-status-request",
  "operation": "default",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/widgets",
      "operation_id": "createWidget"
    },
    "location": "request",
    "field": "status"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/widgets",
      "operation_id": "createWidget"
    },
    "location": "request",
    "field": "status"
  },
  "parameters": {
    "field": "status",
    "value": "active"
  },
  "preconditions": [
    "status may be absent",
    "explicit null is not a missing field"
  ],
  "postconditions": [
    "missing status becomes active",
    "explicit null is not replaced"
  ],
  "evidence": "destination requires status; missing values use an explicit default",
  "lossiness": "default_inserted",
  "reversibility": "irreversible",
  "fixture_refs": [
    "request.positive.json",
    "request.null.json",
    "request.malformed.json",
    "request.present.json"
  ]
}
```

## enum_map

Parameters: `{ "field": "...", "map": { }, "unmapped": "fail" | { "policy": "const", "value": <json> } }`

Missing → `missing`. Null → `explicit_null`. Unmapped scalar with fail → `unknown_enum`. Non-scalar → `malformed`. Map keys are JSON object keys, so they are text: a string value matches the key equal to it, and a number or boolean matches the key equal to its JSON text (`1`, `2.5`, `true`). Type-check: every key must be a source enum member and must match only one (a key `"1"` against a source enum holding both `1` and `"1"` is a type error, because the map cannot tell them apart), every value a destination enum member, and the map must cover the source enum unless the rule sets an `unmapped` policy: an uncovered value with no policy is a type error that blocks approval; with an explicit `unmapped: fail` it is a warning (such inputs fail with `unknown_enum` at verify). A many-to-one map or a const fallback records `enum_many_to_one` when it fires.

Example rule (from `packs/enum-map-color/proposal.json`; also `schemas/v1/examples/valid-enum-map.json`):

```json
{
  "id": "enum-map-color-request",
  "operation": "enum_map",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/swatches",
      "operation_id": "createSwatch"
    },
    "location": "request",
    "field": "color"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/swatches",
      "operation_id": "createSwatch"
    },
    "location": "request",
    "field": "color"
  },
  "parameters": {
    "field": "color",
    "map": {
      "RED": "red",
      "BLUE": "blue",
      "GREEN": "green"
    },
    "unmapped": "fail"
  },
  "preconditions": [
    "color is a mapped source enum value"
  ],
  "postconditions": [
    "color is a destination enum value"
  ],
  "evidence": "uppercase source colors map onto lowercase destination colors",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.unknown.json",
    "request.missing.json",
    "request.null.json"
  ]
}
```

## drop

Parameters: `{ "field": "<name>" }`

Removes the field. Missing fails. Always records `dropped`; reversibility is `irreversible`.

Example rule (from `packs/drop-legacy-flag/proposal.json`; also `schemas/v1/examples/valid-drop.json`):

```json
{
  "id": "drop-legacy-flag-request",
  "operation": "drop",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/flags",
      "operation_id": "createFlag"
    },
    "location": "request",
    "field": "legacy"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/flags",
      "operation_id": "createFlag"
    },
    "location": "request",
    "field": "legacy"
  },
  "parameters": {
    "field": "legacy"
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request drop",
  "lossiness": "dropped",
  "reversibility": "irreversible",
  "fixture_refs": [
    "request.positive.json",
    "request.missing.json"
  ]
}
```

## required_field

Parameters: `{ "field": "<name>" }`

Field must be present and non-null. Missing fails. Explicit null fails.

Example rule (from `packs/require-email/proposal.json`; also `schemas/v1/examples/valid-required-field.json`):

```json
{
  "id": "require-email-request",
  "operation": "required_field",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/mail",
      "operation_id": "createMail"
    },
    "location": "request",
    "field": "email"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/mail",
      "operation_id": "createMail"
    },
    "location": "request",
    "field": "email"
  },
  "parameters": {
    "field": "email"
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request required_field",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.missing.json",
    "request.null.json"
  ]
}
```

## safe_cast

Parameters: `{ "field": "...", "from": "string|integer|number", "to": "string|integer|number", "format": "email"? }`

Overflow, non-finite numbers, and malformed strings fail. Casts keep the exact value or fail (`malformed`, kind `overflow`): string -> number keeps integers as exact JSON integers at any i64/u64 size, and refuses a decimal string that no JSON number holds exactly (`"9007199254740993.0"`, `"1e400"`) instead of rounding it; number -> integer refuses fractions and values outside i64. Optional `format=email` applies the same check as the schema validator: one `@`, a non-empty local part without spaces, and a dotted domain with non-empty labels; any other `format` is rejected at decode. Type-check: `from` must match the declared source type and `to` the declared destination type.

Example rule (from `packs/safe-cast-count/proposal.json`; also `schemas/v1/examples/valid-safe-cast.json`):

```json
{
  "id": "safe-cast-count-request",
  "operation": "safe_cast",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/counts",
      "operation_id": "createCount"
    },
    "location": "request",
    "field": "count"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/counts",
      "operation_id": "createCount"
    },
    "location": "request",
    "field": "count"
  },
  "parameters": {
    "field": "count",
    "from": "string",
    "to": "integer"
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request safe_cast",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.malformed.json",
    "request.overflow.json",
    "request.missing.json"
  ]
}
```

## unit_conversion

Parameters: `{ "field": "...", "from_unit": "...", "to_unit": "...", "factor_num": <i64>, "factor_den": <i64>, "rounding": "exact|floor|ceil|round" }`

Integer arithmetic only. Exact rounding fails when the division has a remainder. Overflow fails. `rounded` is recorded only when a remainder was discarded.

Example rule (from `packs/unit-mass-grams/proposal.json`; also `schemas/v1/examples/valid-unit-conversion.json`):

```json
{
  "id": "unit-mass-grams-request",
  "operation": "unit_conversion",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/mass",
      "operation_id": "createMass"
    },
    "location": "request",
    "field": "mass"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/mass",
      "operation_id": "createMass"
    },
    "location": "request",
    "field": "mass"
  },
  "parameters": {
    "field": "mass",
    "from_unit": "kg",
    "to_unit": "g",
    "factor_num": 1000,
    "factor_den": 1,
    "rounding": "exact"
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request unit_conversion",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.overflow.json",
    "request.missing.json"
  ]
}
```

## split

Parameters: `{ "field": "...", "into": ["a","b"], "separator": " " }`

Splits a string with `splitn(into.len())`. Too few parts → `malformed`. Destination field collision fails, including two `into` names that are equal (rejected at decode and again at runtime). Source field is removed.

Example rule (from `packs/split-full-name/proposal.json`; also `schemas/v1/examples/valid-split.json`):

```json
{
  "id": "split-full-name-request",
  "operation": "split",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/names",
      "operation_id": "createName"
    },
    "location": "request",
    "field": "full_name"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/names",
      "operation_id": "createName"
    },
    "location": "request",
    "field": "first_name"
  },
  "parameters": {
    "field": "full_name",
    "into": [
      "first_name",
      "last_name"
    ],
    "separator": " "
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request split",
  "lossiness": "none",
  "reversibility": "partially_reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.malformed.json",
    "request.missing.json",
    "request.collision.json"
  ]
}
```

## merge

Parameters: `{ "fields": ["a","b"], "into": "c", "separator": " " }`

Concatenates strings. Missing or null source fields fail. Duplicate `fields` are rejected. Collision on `into` fails. Source fields are removed. Always records `merged`.

Example rule (from `packs/merge-display/proposal.json`; also `schemas/v1/examples/valid-merge.json`):

```json
{
  "id": "merge-display-request",
  "operation": "merge",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/people",
      "operation_id": "createPerson"
    },
    "location": "request",
    "field": "first_name"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/people",
      "operation_id": "createPerson"
    },
    "location": "request",
    "field": "display_name"
  },
  "parameters": {
    "fields": [
      "first_name",
      "last_name"
    ],
    "into": "display_name",
    "separator": " "
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request merge",
  "lossiness": "merged",
  "reversibility": "partially_reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.missing.json",
    "request.null.json",
    "request.collision.json"
  ]
}
```

## array_map

Parameters: `{ "field": "...", "map": { }, "unmapped": ... }` as enum_map, applied to each array item: a null item is `explicit_null` at that item's index, a non-scalar item `malformed`. A null field is `explicit_null`. Arrays longer than 256 fail as a bound.

Example rule (from `packs/array-tags/proposal.json`; also `schemas/v1/examples/valid-array-map.json`):

```json
{
  "id": "array-map-request",
  "operation": "array_map",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/tags",
      "operation_id": "createTags"
    },
    "location": "request",
    "field": "tags"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/tags",
      "operation_id": "createTags"
    },
    "location": "request",
    "field": "tags"
  },
  "parameters": {
    "field": "tags",
    "map": {
      "RED": "red",
      "BLUE": "blue",
      "GREEN": "green"
    },
    "unmapped": "fail"
  },
  "preconditions": [
    "tags are mapped enums"
  ],
  "postconditions": [
    "destination enums"
  ],
  "evidence": "map tag enums",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.unknown.json",
    "request.missing.json",
    "request.malformed.json"
  ]
}
```

## array_filter

Parameters: `{ "field": "...", "drop_null": true }`

Removes JSON null items when `drop_null` is true. Missing field → `missing`; a null field → `explicit_null` (as for every other operation); any other non-array value → `malformed`. Records `dropped` when at least one item was removed.

Example rule (from `packs/array-tags/proposal.json`; also `schemas/v1/examples/valid-array-filter.json`):

```json
{
  "id": "array-filter-request",
  "operation": "array_filter",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/tags",
      "operation_id": "createTags"
    },
    "location": "request",
    "field": "tags"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/tags",
      "operation_id": "createTags"
    },
    "location": "request",
    "field": "tags"
  },
  "parameters": {
    "field": "tags",
    "drop_null": true
  },
  "preconditions": [
    "tags is an array"
  ],
  "postconditions": [
    "null items removed"
  ],
  "evidence": "filter null tags",
  "lossiness": "dropped",
  "reversibility": "irreversible",
  "fixture_refs": [
    "request.positive.json",
    "request.null-item.json"
  ]
}
```

## pagination

Parameters: `{ "from_items": "results", "from_token": "next", "to_data": "data", "to_cursor": "cursor" }`

Renames the page list and token. Collision and missing behave as rename.

Example rule (from `packs/page-envelope/proposal.json`; also `schemas/v1/examples/valid-pagination.json`):

```json
{
  "id": "page-envelope-request",
  "operation": "pagination",
  "source": {
    "operation": {
      "method": "POST",
      "path": "/pages",
      "operation_id": "listPages"
    },
    "location": "request",
    "field": "results"
  },
  "destination": {
    "operation": {
      "method": "POST",
      "path": "/pages",
      "operation_id": "listPages"
    },
    "location": "request",
    "field": "data"
  },
  "parameters": {
    "from_items": "results",
    "from_token": "next",
    "to_data": "data",
    "to_cursor": "cursor"
  },
  "preconditions": [
    "typed against source schema"
  ],
  "postconditions": [
    "typed against destination schema"
  ],
  "evidence": "request pagination",
  "lossiness": "none",
  "reversibility": "reversible",
  "fixture_refs": [
    "request.positive.json",
    "request.missing.json",
    "request.collision.json"
  ]
}
```

## Common rule fields

Every rule records source and destination operation identity, location (`request` or `response`), evidence, declared `lossiness` (`none | default_inserted | enum_many_to_one | dropped | rounded | merged`), `reversibility` (`reversible | irreversible | partially_reversible`), pre/postconditions, and fixture references. Values outside those vocabularies are type errors.

Proposals are not executable. They cannot carry `approval` fields. `cfp review` type-checks rules against both contracts and writes every diagnostic; `cfp approve` refuses a proposal that has any error-severity diagnostic.

## Limits

At most 32 rules per proposal. Arrays at most 256 items. No dynamic code evaluation.
