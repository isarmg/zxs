"""Independent SQLite schema fixture, not a product or a filesystem writer.

Future qualification only. Uses TEMPORARY IN-MEMORY DATABASES for relational
invariants; deliberately cannot prove physical media or power-loss behavior.
Run only when separately authorized with an upstream-qualified SQLite engine.
"""
import hashlib
import sqlite3
from pathlib import Path

BASE = Path(__file__).resolve().parents[2] / 'src' / 'system'


def statement(name):
    return (BASE / name).read_text(encoding='utf-8')


def fixture():
    lines = (Path(__file__).parent / 'init-v1.txt').read_text('ascii').splitlines()
    header = dict(line.split('=', 1) for line in lines if '=' in line and not line.startswith('#'))
    rows = [line.split() for line in lines if line and line[0].isdigit()]
    blob = bytes.fromhex
    op, actor, approval, pool = (blob(header[k]) for k in ('operation','actor','approval_ref','pool_result'))
    uid = int(header['storage_uid'])
    paths = [(int(i),int(role),blob(path),blob(vol),int(limit),blob(token),blob(mark))
             for i, role, path, vol, limit, token, mark in rows]
    request = b'ZXS-SQLITE-1/INIT\0' + op + actor + approval + uid.to_bytes(4,'big') + len(paths).to_bytes(2,'big')
    for _, role, path, vol, _, token, mark in paths:
        request += bytes([role]) + len(path).to_bytes(2,'big') + path
        assert mark == b'ZXS-SQLITE-1/VOLUME\0' + bytes([role]) + pool + vol + op + token
    assert request == blob(header['request_input'])
    digest = hashlib.sha256(request).digest()
    assert digest == blob(header['request_sha'])
    return op, actor, approval, uid, pool, digest, paths


def refused(db, sql, bindings=()):
    try:
        db.execute(sql, bindings).fetchall()
    except sqlite3.IntegrityError:
        return
    raise AssertionError('expected a database constraint failure')


def main():
    op, actor, approval, uid, pool, digest, rows = fixture()
    c = sqlite3.connect(':memory:', isolation_level=None)
    try:
        c.execute('PRAGMA foreign_keys=ON')
        c.execute('PRAGMA trusted_schema=OFF')
        assert c.execute('PRAGMA foreign_keys').fetchone() == (1,)
        assert c.execute('PRAGMA trusted_schema').fetchone() == (0,)
        # executescript commits a prior transaction: BEGIN inside the script.
        c.executescript('BEGIN IMMEDIATE;\n' + statement('schema.sql'))
        insert_claim = 'INSERT INTO volume_claim VALUES (?,?,?,?,?,?,?)'
        c.execute('INSERT INTO operation VALUES(?,?,?,?,?)',(op,1,actor,digest,0))
        c.execute('INSERT INTO pool_birth VALUES(1,?,?,?,?,?)',(pool,op,approval,uid,len(rows)))
        finish = statement('complete-initial.sql')
        args = (op,actor,digest,pool,approval)
        for i,role,path,vol_id,limit,token,_ in rows:
            if i==3:
                refused(c,finish,args)   # missing last row; negative before complete group
                refused(c,insert_claim,(op,i,role,vol_id,b'/mnt/f-a/sub',limit,token))
                refused(c,insert_claim,(op,i,role,vol_id,b'/mnt/f-0',limit,token))
            c.execute(insert_claim,(op,i,role,vol_id,path,limit,token))
        c.execute('COMMIT')             # a genuine pending result, not active
        lookup = statement('original-init.sql')
        assert c.execute(lookup,(op,actor,digest)).fetchall()==[(pool,0)]
        assert c.execute(lookup,(op,b'x'*16,digest)).fetchall()==[]
        declared = c.execute(statement('initial-catalog.sql'),(pool,)).fetchall()
        assert len(declared)==4 and declared[-1][11]==bytes.fromhex('2f6d6e742f662dff')
        assert c.execute(statement('initial-catalog.sql'),(b'x'*16,)).fetchall()==[]
        refused(c,'INSERT OR REPLACE INTO operation VALUES(?,?,?,?,?)',(op,1,actor,digest,0))
        refused(c,'UPDATE volume_claim SET claim_token=? WHERE ordinal=1',(b'x'*32,))
        c.execute('BEGIN IMMEDIATE')
        assert c.execute(finish,(op,actor,b'x'*32,pool,approval)).fetchall()==[]
        assert c.execute(finish,args).fetchall()==[(op,)]
        c.execute('ROLLBACK')           # executing RETURNING is not committing
        assert c.execute(lookup,(op,actor,digest)).fetchall()==[(pool,0)]
        c.execute('BEGIN IMMEDIATE')
        assert c.execute(finish,args).fetchall()==[(op,)]
        c.execute('COMMIT')
        assert c.execute(lookup,(op,actor,digest)).fetchall()==[(pool,1)]
        assert c.execute(finish,args).fetchall()==[]    # retry takes original result
        refused(c,'UPDATE operation SET state=0 WHERE op_id=?',(op,))
        refused(c,'DELETE FROM volume_claim WHERE ordinal=0')
        assert c.execute('PRAGMA foreign_key_check').fetchall()==[]
    finally:
        c.close()


if __name__ == '__main__':
    main()
