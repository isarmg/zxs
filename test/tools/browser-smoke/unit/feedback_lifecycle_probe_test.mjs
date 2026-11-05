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
assert.ok(scene.includes('writes.length===8&&writes.every'));
assert.ok(scene.includes("if(!response.ok)throw new Error('real default branch publication failed')"));
assert.ok(scene.includes("typeof window.__zxsNoticeDelivery.release==='function'"));
console.log(`PASS ${cases} real-copy and settled-notice observer model cases; no browser publication executed`);
