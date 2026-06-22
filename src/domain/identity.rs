use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::schema::identities;

#[derive(Queryable, Selectable, Identifiable, Debug, Clone)]
#[diesel(table_name = identities)]
#[diesel(primary_key(uuid))]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Identity {
    pub uuid: String,
    pub user_id: Option<String>,
    pub ip_address: String,
    pub device_token: Option<String>,
    pub server_private_key: Vec<u8>,
    pub server_public_key: Vec<u8>,
    pub client_public_key: Vec<u8>,
    pub extra: String,
    pub is_confirmed: bool,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = identities)]
pub struct NewIdentity {
    pub uuid: String,
    pub user_id: Option<String>,
    pub ip_address: String,
    pub device_token: Option<String>,
    pub server_private_key: Vec<u8>,
    pub server_public_key: Vec<u8>,
    pub client_public_key: Vec<u8>,
    pub extra: String,
    pub is_confirmed: bool,
}

#[derive(AsChangeset, Debug)]
#[diesel(table_name = identities)]
pub struct UpdateIdentity {
    pub user_id: Option<String>,
    pub ip_address: Option<String>,
    pub device_token: Option<String>,
    pub client_public_key: Option<Vec<u8>>,
    pub extra: Option<String>,
    pub is_confirmed: Option<bool>,
}

impl Identity {
    pub fn find_by_ip(conn: &mut SqliteConnection, ip: &str) -> QueryResult<Option<Self>> {
        identities::table
            .filter(identities::ip_address.eq(ip))
            .first(conn)
            .optional()
    }

    pub fn find_by_device_token(
        conn: &mut SqliteConnection,
        token: &str,
    ) -> QueryResult<Option<Self>> {
        identities::table
            .filter(identities::device_token.eq(token))
            .first(conn)
            .optional()
    }

    pub fn find_by_uuid(conn: &mut SqliteConnection, uuid: &str) -> QueryResult<Option<Self>> {
        identities::table.find(uuid).first(conn).optional()
    }

    pub fn find_all(conn: &mut SqliteConnection) -> QueryResult<Vec<Self>> {
        identities::table.load(conn)
    }

    pub fn create(conn: &mut SqliteConnection, new: NewIdentity) -> QueryResult<Self> {
        diesel::insert_into(identities::table)
            .values(&new)
            .returning(Self::as_select())
            .get_result(conn)
    }

    pub fn update(
        conn: &mut SqliteConnection,
        uuid: &str,
        changes: UpdateIdentity,
    ) -> QueryResult<Self> {
        let now = chrono::Utc::now().naive_utc();
        diesel::update(identities::table.find(uuid))
            .set((&changes, identities::updated_at.eq(now)))
            .returning(Self::as_select())
            .get_result(conn)
    }

    pub fn find_pending(conn: &mut SqliteConnection) -> QueryResult<Vec<Self>> {
        identities::table
            .filter(identities::is_confirmed.eq(false))
            .load(conn)
    }

    pub fn delete(conn: &mut SqliteConnection, uuid: &str) -> QueryResult<usize> {
        diesel::delete(identities::table.find(uuid)).execute(conn)
    }
}
