use chrono::NaiveDateTime;
use diesel::prelude::*;

use crate::schema::passwords;

#[derive(Queryable, Selectable, Identifiable, Associations, Debug, Clone)]
#[diesel(table_name = passwords)]
#[diesel(primary_key(uuid))]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
#[diesel(belongs_to(crate::domain::group::Group, foreign_key = group_id))]
pub struct Password {
    pub uuid: String,
    pub group_id: String,
    pub pwd: String,
    pub name: String,
    pub extra: String,
    pub valid_since_days: i32,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub valid_since: NaiveDateTime,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = passwords)]
pub struct NewPassword {
    pub uuid: String,
    pub group_id: String,
    pub pwd: String,
    pub name: String,
    pub extra: String,
    pub valid_since_days: i32,
    pub valid_since: NaiveDateTime,
}

#[derive(AsChangeset, Debug)]
#[diesel(table_name = passwords)]
pub struct UpdatePassword {
    pub pwd: Option<String>,
    pub group_id: Option<String>,
    pub name: Option<String>,
    pub extra: Option<String>,
    pub valid_since_days: Option<i32>,
}

impl Password {
    pub fn find_by_uuid(conn: &mut SqliteConnection, uuid: &str) -> QueryResult<Option<Self>> {
        passwords::table.find(uuid).first(conn).optional()
    }

    pub fn find_by_groups(
        conn: &mut SqliteConnection,
        group_ids: &[String],
    ) -> QueryResult<Vec<Self>> {
        passwords::table
            .filter(passwords::group_id.eq_any(group_ids))
            .load(conn)
    }

    pub fn create(conn: &mut SqliteConnection, new: NewPassword) -> QueryResult<Self> {
        diesel::insert_into(passwords::table)
            .values(&new)
            .returning(Self::as_select())
            .get_result(conn)
    }

    pub fn update(
        conn: &mut SqliteConnection,
        uuid: &str,
        changes: UpdatePassword,
    ) -> QueryResult<Self> {
        let now = chrono::Utc::now().naive_utc();
        diesel::update(passwords::table.find(uuid))
            .set((
                &changes,
                passwords::updated_at.eq(now),
                passwords::valid_since.eq(now),
            ))
            .returning(Self::as_select())
            .get_result(conn)
    }
}
