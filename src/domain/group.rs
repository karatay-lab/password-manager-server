use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::schema::groups;

#[derive(Queryable, Selectable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = groups)]
#[diesel(primary_key(uuid))]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
#[diesel(belongs_to(crate::domain::user::User, foreign_key = user_id))]
pub struct Group {
    pub uuid: String,
    pub user_id: String,
    pub name: String,
    pub extra: String,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = groups)]
pub struct NewGroup {
    pub uuid: String,
    pub user_id: String,
    pub name: String,
    pub extra: String,
}

impl Group {
    pub fn find_by_uuid(conn: &mut SqliteConnection, uuid: &str) -> QueryResult<Option<Self>> {
        groups::table.find(uuid).first(conn).optional()
    }

    pub fn find_by_user(conn: &mut SqliteConnection, user_id: &str) -> QueryResult<Vec<Self>> {
        groups::table.filter(groups::user_id.eq(user_id)).load(conn)
    }

    pub fn create(conn: &mut SqliteConnection, new: NewGroup) -> QueryResult<Self> {
        diesel::insert_into(groups::table)
            .values(&new)
            .returning(Self::as_select())
            .get_result(conn)
    }
}
