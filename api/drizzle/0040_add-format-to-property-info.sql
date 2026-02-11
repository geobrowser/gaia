-- Add 'format' and 'is_type' fields to property_info composite type and update build_property_info
--
-- FORMAT_PROPERTY_ID = '396f8c72-dfd0-4b57-91ea-09c1b9321b2f'
-- IS_TYPE_PROPERTY_ID = 'd2c1a101-14e3-464a-8272-f4e75b0f1407'

ALTER TYPE property_info ADD ATTRIBUTE format text;
ALTER TYPE property_info ADD ATTRIBUTE is_type boolean;

CREATE OR REPLACE FUNCTION public.build_property_info(entity_id uuid)
RETURNS property_info AS $$
  SELECT ROW(
    entity_id,
    -- name
    (SELECT "text" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1),
    -- description
    (SELECT "text" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = '9b1f76ff-9711-404c-861e-59dc3fa7d037' LIMIT 1),
    -- data_type_id (via DATA_TYPE relation)
    (SELECT r.to_entity_id FROM relations r
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '6d29d578-49bb-4959-baf7-2cc696b1671a' LIMIT 1),
    -- data_type_name
    (SELECT v."text" FROM "values" v
     JOIN relations r ON r.to_entity_id = v.entity_id
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '6d29d578-49bb-4959-baf7-2cc696b1671a'
     AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1),
    -- renderable_type_id (via RENDERABLE_TYPE relation)
    (SELECT r.to_entity_id FROM relations r
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46' LIMIT 1),
    -- renderable_type_name
    (SELECT v."text" FROM "values" v
     JOIN relations r ON r.to_entity_id = v.entity_id
     WHERE r.from_entity_id = build_property_info.entity_id
     AND r.type_id = '2316bbe1-c76f-4635-83f2-3e03b4f1fe46'
     AND v.property_id = 'a126ca53-0c8e-48d5-b888-82c734c38935' LIMIT 1),
    -- format
    (SELECT "text" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = '396f8c72-dfd0-4b57-91ea-09c1b9321b2f' LIMIT 1),
    -- is_type
    (SELECT "boolean" FROM "values" WHERE entity_id = build_property_info.entity_id
     AND property_id = 'd2c1a101-14e3-464a-8272-f4e75b0f1407' LIMIT 1)
  )::property_info;
$$ LANGUAGE sql STABLE;
