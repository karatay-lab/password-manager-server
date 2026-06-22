diesel::table! {
    users (uuid) {
        uuid -> Text,
        name -> Text,
        ehlo_secret -> Text,
        is_deleted -> Bool,
        extra -> Text,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    identities (uuid) {
        uuid -> Text,
        user_id -> Nullable<Text>,
        ip_address -> Text,
        device_token -> Nullable<Text>,
        server_private_key -> Binary,
        server_public_key -> Binary,
        client_public_key -> Binary,
        extra -> Text,
        is_confirmed -> Bool,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    groups (uuid) {
        uuid -> Text,
        user_id -> Text,
        name -> Text,
        extra -> Text,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    passwords (uuid) {
        uuid -> Text,
        group_id -> Text,
        pwd -> Text,
        name -> Text,
        extra -> Text,
        valid_since_days -> Integer,
        created_at -> Timestamp,
        updated_at -> Timestamp,
        valid_since -> Timestamp,
    }
}

diesel::joinable!(identities -> users (user_id));
diesel::joinable!(groups -> users (user_id));
diesel::joinable!(passwords -> groups (group_id));

diesel::allow_tables_to_appear_in_same_query!(users, identities, groups, passwords);
