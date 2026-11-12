// Execute the actual browser observation expression with bounded host objects.
// This checks the probe, not production DOM rendering or a real publication.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const scene = readFileSync(new URL('../runner/feedback_lifecycle.rs', import.meta.url), 'utf8');
const production = readFileSync(new URL('../../../../src/web/src/files/actions/mutation.rs', import.meta.url), 'utf8');
const constant = name => {
    const match = scene.match(new RegExp(`const ${name}: &str = ("[^"\\n]*");`));
    assert.ok(match, `missing ${name} literal`);
    return JSON.parse(match[1]);
};
const messages = ['DEFAULT_SUCCESS_EN', 'DEFAULT_SUCCESS_ZH'].map(constant);
for (const message of messages) assert.ok(production.includes(JSON.stringify(message)),
    `saved-state probe must match the real success text: ${message}`);
const saved = scene.slice(scene.indexOf('fn saved('), scene.indexOf('fn exercise_notices('));
assert.ok(saved.includes('messages = json!([DEFAULT_SUCCESS_EN, DEFAULT_SUCCESS_ZH])'));
assert.ok(saved.includes('notice_selector = json!(NOTICE)'));
assert.ok(saved.includes('default_selector = json!(DEFAULT)'));
const template = saved.match(/r#"([\s\S]*?)"#/);
assert.ok(template, 'missing saved-state browser expression');
const expression = template[1]
    .replace('{notice_selector}', JSON.stringify(constant('NOTICE')))
    .replace('{default_selector}', JSON.stringify(constant('DEFAULT')))
    .replace('{messages}', JSON.stringify(messages));
assert.ok(!expression.includes('{'), 'unexpected unbound format placeholder');
const observe = new Function('document', expression);
let cases = 0;
const expect = (label, {message = messages[0], role = 'status', disabled = true,
    notice = true, button = true} = {}, expected = false) => {
    const document = {querySelector(selector) {
        if (selector === constant('NOTICE')) return notice ? {
            textContent: message, getAttribute: name => name === 'role' ? role : null,
        } : null;
        if (selector === constant('DEFAULT')) return button ? {disabled} : null;
        throw new Error(`unexpected selector: ${selector}`);
    }};
    assert.equal(Boolean(observe(document)), expected, label);
    cases++;
};
for (const message of messages) {
    expect('real published success', {message}, true);
    expect('success surrounded only by whitespace', {message: `\n${message} `}, true);
    expect('an enabled publish button is not settled', {message, disabled: false});
    expect('an alert is not a success notice', {message, role: 'alert'});
    expect('a missing publish control is not settled', {message, button: false});
}
for (const message of ['Default branch saved', '已设为默认分支', 'Publication pending', '',
    `${messages[0]} Another operation`, `Unexpected prefix ${messages[1]}`]) {
    expect('only the actual complete success text is accepted', {message});
}
expect('no notice is not success', {notice: false});
expect('missing role is not success', {role: null});
assert.ok(scene.includes('writes.length===11&&writes.every'));
const navigation = readFileSync(new URL('../runner/admin_navigation.rs', import.meta.url), 'utf8');
const pages = navigation.match(/const ALL: \[Self; 4\] = \[([\s\S]*?)\];/);
assert.ok(pages, 'missing current management navigation declaration');
assert.deepEqual([...pages[1].matchAll(/Self::(\w+)/g)].map(match => match[1]),
    ['Storage', 'Repositories', 'Rules', 'Diagnostics']);
assert.ok(scene.includes('.chain(AdminPage::ALL.map(AdminPage::labels))'));
assert.ok(scene.includes('[("Status", "状态"), ("History", "历史记录")]'));
assert.ok(scene.includes('.chain([("Account settings", "账号设置"), ("Details", "详情")])'));
const publication = scene.match(/^\s*("const writes=window\.__zxsNoticeDelivery\.writes;return [^"\n]*"),$/m);
assert.ok(publication, 'missing final real-publication browser predicate');
const verifyPublications = new Function('window', JSON.parse(publication[1]));
const writes = count => Array.from({length: count}, (_, index) => ({
    method: 'PUT', path: `/api/file-manager/files/file-${index}/default-branch`,
}));
let publicationCases = 0;
const expectPublications = (label, actual, expected) => {
    assert.equal(Boolean(verifyPublications({__zxsNoticeDelivery: {writes: actual}})),
        expected, label);
    publicationCases++;
};
expectPublications('eight navigation targets plus dismiss, expiry and late delivery', writes(11), true);
for (const count of [0, 8, 10, 12]) {
    expectPublications(`wrong publication count ${count}`, writes(count), false);
}
for (let index = 0; index < 11; index++) {
    for (const change of [
        {method: 'POST'}, {method: 'GET'}, {method: 'DELETE'},
        {path: '/api/repositories/repo/branches'},
        {path: '/api/file-manager/files//default-branch'},
        {path: '/api/file-manager/files/file/default-branch/outcome'},
        {path: '/api/repositories/repo/default-branch'},
    ]) {
        const actual = writes(11);
        Object.assign(actual[index], change);
        expectPublications(`wrong publication at ${index}: ${JSON.stringify(change)}`, actual, false);
    }
}
assert.equal(publicationCases, 82);
assert.ok(scene.includes("if(!response.ok)throw new Error('real default branch publication failed')"));
assert.ok(scene.includes("typeof window.__zxsNoticeDelivery.release==='function'"));
console.log(`PASS ${cases} real-copy and settled-notice observer model cases; ${publicationCases} exact-publication predicate cases; no browser publication executed`);
