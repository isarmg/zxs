impl WebAdmin<'_> {
    pub(super) fn id(&self)->io::Result<AccountId>{self.source.at(|db,c|{self.current(db,&c)?;Ok(self.account)})}
}

#[test]
fn web_credentials_and_sqlite_recovery_remain_distinct(){
    let rejection=denied();
    assert!(credentials_refused(&rejection));
    let recovery=check(776).unwrap_err(); // SQLITE_READONLY_ROLLBACK
    assert_eq!(recovery.kind(),rejection.kind());
    assert!(!credentials_refused(&recovery));
}
