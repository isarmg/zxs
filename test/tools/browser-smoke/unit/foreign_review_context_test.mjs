// Host adapters exercise the DOM collector and checker, not a rendered
// browser, accessibility tree or screen reader. No requests or receipts are made.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const collectSource = readFileSync(new URL('../runner/foreign_review/accessibility_snapshot.js', import.meta.url), 'utf8');
const validate = new Function(readFileSync(new URL('../runner/foreign_review/accessibility_model.js', import.meta.url), 'utf8'))();

function node(tag, text = '', attributes = {}) {
  return {tagName:tag, textContent:text, attributes, disabled:false, checked:false, tabIndex:0,
    dataset:{}, one:new Map(), many:new Map(), labels:[], descendants:new Set(),
    contains(child) { return this === child || this.descendants.has(child); },
    getAttribute(name) { return this.attributes[name] ?? null; },
    getClientRects() { return [{}]; },
    querySelector(selector) { return this.one.get(selector) ?? null; },
    querySelectorAll(selector) { return this.many.get(selector) ?? []; }};
}
function fixture(phase = 'draft', form = true, english = true) {
  const ids = new Map();
  const root = node('SECTION', '', {'aria-label':english
    ? 'Foreign history reconstruction review wizard' : '外来历史重建审核向导'});
  const anchor = node('SECTION'); anchor.closest = () => root;
  if (form) root.one.set('[data-foreign-plan-form]', anchor);
  const expectedGroups = [
    {id:'1'.repeat(64), history:'history-one', origin:'disk-origin-one', missing:1},
    {id:'2'.repeat(64), history:'history-two', origin:'disk-origin-two', missing:3},
  ];
  const controls = [], groups = [];
  for (const expected of form ? expectedGroups : []) {
    const prefix = `zxs-foreign-plan-${expected.id}`;
    const references = [`${prefix}-origin`, `${prefix}-history`, `${prefix}-availability`];
    const input = node('INPUT', '', {'aria-describedby':references.join(' ')});
    input.disabled = phase === 'unknown';
    input.labels = [node('LABEL', english ? 'Distinct destination child name' : '独立目标子目录名')];
    ids.set(references[0], node('P', `P01 · Same repository · ${expected.origin}`));
    ids.set(references[1], node('P', expected.history));
    ids.set(references[2], node('P', `Contents temporarily unavailable; history retained: ${expected.missing}`));
    ids.get(references[2]).dataset.foreignRetainedMissing = String(expected.missing);
    const group = node('SECTION', references.map(id => ids.get(id).textContent).join(' '));
    for (const id of references) {
      group.descendants.add(ids.get(id));
      root.descendants.add(ids.get(id));
    }
    group.dataset.foreignPlanGroup = expected.id;
    group.one.set('input', input);
    const availability = node('P'); availability.dataset.foreignRetainedMissing = String(expected.missing);
    group.one.set('[data-foreign-retained-missing]', availability);
    controls.push(input); groups.push(group);
  }
  root.many.set('[data-foreign-plan-group]', groups);
  root.many.set('[data-foreign-plan-mode]', form
    ? ['true', 'false', 'false'].map(pressed => node('BUTTON', 'Intent', {'aria-pressed':pressed})) : []);
  function control(selector, text, disabled = false, tag = 'BUTTON') {
    const value = node(tag, text);
    if (tag === 'INPUT') value.labels = [node('LABEL', text)];
    value.disabled = disabled; root.one.set(selector, value); controls.push(value); return value;
  }
  control('[data-foreign-plan-preview]', 'Review mapping', phase === 'unknown' || phase === 'local');
  const reviewed = ['review', 'unknown', 'stored'].includes(phase);
  if (reviewed) {
    root.one.set('[data-foreign-plan-review]', anchor);
    control('[data-foreign-plan-execution-unavailable]', 'Import execution is unavailable', true);
    ids.set('zxs-foreign-plan-operation', node('P', 'Review operation ID: op_original'));
    ids.set('zxs-foreign-plan-scope', node('P', 'No target is reserved.'));
    root.descendants.add(ids.get('zxs-foreign-plan-operation'));
    root.descendants.add(ids.get('zxs-foreign-plan-scope'));
  }
  const description = 'zxs-foreign-plan-operation zxs-foreign-plan-scope';
  if (phase === 'review') {
    control('[data-foreign-plan-confirm]', english
      ? 'Save only the system-disk review record' : '仅同意保存系统盘审核记录', false, 'INPUT')
      .attributes['aria-describedby'] = description;
    control('[data-foreign-plan-save]', 'Save review plan only', true).attributes['aria-describedby'] = description;
  }
  if (phase === 'unknown' || phase === 'stored') {
    control('[data-foreign-plan-status]', 'Query original review record').attributes['aria-describedby'] = description;
    if (phase === 'unknown') {
      control('[data-foreign-plan-retry]', 'Retry original review save').attributes['aria-describedby'] = description;
      root.many.set('[role=alert]', [node('P', 'Outcome unknown. Keep the original ID.')]);
    } else {
      control('[data-foreign-plan-revalidate]', 'Revalidate original source and target').attributes['aria-describedby'] = description;
      root.one.set('[data-foreign-plan-saved] [role=status]', node('P', english
        ? 'Saved review; this is not an import completion receipt' : '审核计划已保存；这不是迁入完成回执'));
    }
  }
  root.many.set('button,input,select,textarea', controls);
  const document = {
    querySelector() { return anchor; }, querySelectorAll() { return [root]; },
    getElementById(id) { return ids.get(id) ?? null; },
  };
  const collect = new Function('document', `return function(){${collectSource}}`)(document);
  return {root, ids, groups, controls, expectedGroups, snapshot:() => collect({phase, expectedGroups})};
}

let cases = 0;
function test(name, action) { action(); cases++; console.log(`PASS ${name}`); }
for (const english of [true, false]) {
  test(`separate origins and unavailable bodies have named linked context (${english ? 'English' : 'Chinese'})`, () => {
    const f = fixture('draft', true, english), snapshot = f.snapshot();
    assert.deepEqual(validate(snapshot), []);
    assert.deepEqual(snapshot.groups.map(group => group.id), f.expectedGroups.map(group => group.id));
    assert.deepEqual(snapshot.groups.map(group => group.missing), ['1', '3']);
    assert.ok(snapshot.groups.every(group => group.linkedContext && group.namedInput));
    assert.ok(snapshot.controls.every(control => control.brokenReferences.length === 0));
  });
  test(`plan-only consent and original-operation descriptions remain valid (${english ? 'English' : 'Chinese'})`, () => {
    const f = fixture('review', true, english);
    assert.deepEqual(validate(f.snapshot()), []);
    f.ids.delete('zxs-foreign-plan-operation');
    assert.ok(validate(f.snapshot()).some(error => error.includes('broken ARIA')));
  });
}
for (const field of ['origin', 'history', 'availability']) {
  test(`missing ${field} description is reported as a broken reference`, () => {
    const f = fixture(); f.ids.delete(`zxs-foreign-plan-${f.expectedGroups[0].id}-${field}`);
    assert.ok(validate(f.snapshot()).some(error => error.includes('broken ARIA')));
  });
}
test('removing every description link cannot leave a passing history mapping', () => {
  const f = fixture();
  delete f.groups[0].querySelector('input').attributes['aria-describedby'];
  assert.notDeepEqual(validate(f.snapshot()), []);
});
test('another history with valid IDs cannot describe the current history input', () => {
  const f = fixture();
  f.groups[0].querySelector('input').attributes['aria-describedby'] =
    f.groups[1].querySelector('input').attributes['aria-describedby'];
  assert.notDeepEqual(validate(f.snapshot()), []);
});
test('removing the history input reports the missing control', () => {
  const f = fixture();
  f.groups[0].one.delete('input');
  assert.notDeepEqual(validate(f.snapshot()), []);
});
for (const field of ['origin', 'history', 'availability']) {
  test(`a linked ${field} outside its history group is not accepted`, () => {
    const f = fixture();
    const target = f.ids.get(`zxs-foreign-plan-${f.expectedGroups[0].id}-${field}`);
    f.groups[0].descendants.delete(target);
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
  test(`repeated ${field} references do not count as exact linked context`, () => {
    const f = fixture();
    f.groups[0].querySelector('input').attributes['aria-describedby'] +=
      ` zxs-foreign-plan-${f.expectedGroups[0].id}-${field}`;
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
}
for (const field of ['origin', 'history']) {
  test(`visible group text cannot hide incorrect linked ${field} evidence`, () => {
    const f = fixture();
    f.ids.get(`zxs-foreign-plan-${f.expectedGroups[0].id}-${field}`).textContent = 'different evidence';
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
}
test('the linked unavailable-body count must match the actual source inventory', () => {
  const f = fixture();
  f.ids.get(`zxs-foreign-plan-${f.expectedGroups[0].id}-availability`)
    .dataset.foreignRetainedMissing = '0';
  assert.notDeepEqual(validate(f.snapshot()), []);
});
for (const selector of ['confirm', 'save', 'retry', 'status', 'revalidate']) {
  const phase = ['confirm', 'save'].includes(selector) ? 'review'
    : selector === 'retry' ? 'unknown' : 'stored';
  for (const wrong of [false, true]) {
    test(`${selector} cannot omit original-review context or use unrelated valid IDs (${wrong})`, () => {
      const f = fixture(phase);
      const control = f.root.querySelector(`[data-foreign-plan-${selector}]`);
      if (wrong) {
        control.attributes['aria-describedby'] =
          f.groups[0].querySelector('input').attributes['aria-describedby'];
      } else {
        delete control.attributes['aria-describedby'];
      }
      assert.notDeepEqual(validate(f.snapshot()), []);
    });
  }
}
for (const field of ['operation', 'scope']) {
  test(`recovery cannot use the ${field} description outside its live wizard`, () => {
    const f = fixture('stored', false);
    f.root.descendants.delete(f.ids.get(`zxs-foreign-plan-${field}`));
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
  test(`recovery cannot use an empty ${field} description`, () => {
    const f = fixture('stored', false);
    f.ids.get(`zxs-foreign-plan-${field}`).textContent = ' ';
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
}
test('matching names or slots cannot merge histories', () => {
  const f = fixture(); f.groups[1].dataset.foreignPlanGroup = f.groups[0].dataset.foreignPlanGroup;
  assert.notDeepEqual(validate(f.snapshot()), []);
});
test('a missing body cannot silently become zero unavailable contents', () => {
  const f = fixture(); f.groups[1].querySelector('[data-foreign-retained-missing]').dataset.foreignRetainedMissing = '0';
  assert.notDeepEqual(validate(f.snapshot()), []);
});
for (const form of [true, false]) {
  test(`unknown save preserves original-query and retry controls with form=${form}`, () => {
    const f = fixture('unknown', form); assert.deepEqual(validate(f.snapshot()), []);
    f.root.querySelector('[data-foreign-plan-retry]').disabled = true;
    assert.notDeepEqual(validate(f.snapshot()), []);
  });
}
test('recovery never announces intent saved or copy sealed as import success', () => {
  const f = fixture('stored', false); assert.deepEqual(validate(f.snapshot()), []);
  f.root.querySelector('[data-foreign-plan-saved] [role=status]').textContent = 'Import completed';
  assert.notDeepEqual(validate(f.snapshot()), []);
});
console.log(`PASS ${cases} foreign-review context collector/checker host cases; no rendered DOM or screen-reader acceptance`);
