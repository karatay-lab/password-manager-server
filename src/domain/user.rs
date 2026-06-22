use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::schema::users;

#[derive(Queryable, Selectable, Identifiable, Debug, Clone)]
#[diesel(table_name = users)]
#[diesel(primary_key(uuid))]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct User {
    pub uuid: String,
    pub name: String,
    pub ehlo_secret: String,
    pub is_deleted: bool,
    pub extra: String,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = users)]
pub struct NewUser {
    pub uuid: String,
    pub name: String,
    pub ehlo_secret: String,
    pub is_deleted: bool,
    pub extra: String,
}

#[derive(AsChangeset, Debug)]
#[diesel(table_name = users)]
pub struct UpdateUser {
    pub is_deleted: Option<bool>,
    pub extra: Option<String>,
}

impl User {
    pub fn find_by_uuid(conn: &mut SqliteConnection, uuid: &str) -> QueryResult<Option<Self>> {
        users::table.find(uuid).first(conn).optional()
    }

    pub fn find_by_name(conn: &mut SqliteConnection, name: &str) -> QueryResult<Option<Self>> {
        users::table
            .filter(users::name.eq(name))
            .first(conn)
            .optional()
    }

    pub fn find_all(conn: &mut SqliteConnection) -> QueryResult<Vec<Self>> {
        users::table.load(conn)
    }

    pub fn create(conn: &mut SqliteConnection, new: NewUser) -> QueryResult<Self> {
        diesel::insert_into(users::table)
            .values(&new)
            .returning(Self::as_select())
            .get_result(conn)
    }

    pub fn update(
        conn: &mut SqliteConnection,
        uuid: &str,
        changes: UpdateUser,
    ) -> QueryResult<Self> {
        let now = chrono::Utc::now().naive_utc();
        diesel::update(users::table.find(uuid))
            .set((&changes, users::updated_at.eq(now)))
            .returning(Self::as_select())
            .get_result(conn)
    }
}
