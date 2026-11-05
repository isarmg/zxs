// Real DOM observations only. No focus(), click(), mock attributes or AX claims.
const options = arguments[0];
const anchor = document.querySelector('[data-foreign-plan-form], [data-foreign-plan-review]');
if (!anchor) throw new Error('no live foreign review form or saved review');
const root = anchor.closest('section[aria-label]');
if (!root) throw new Error('foreign review is outside its named landmark');
const text = node => node ? node.textContent.trim() : '';
function referenced(node, attribute) {
  const value = node.getAttribute(attribute);
  return value ? value.trim().split(/\s+/).filter(Boolean) : [];
}
function name(node) {
  if (!node) return '';
  const labelled = referenced(node, 'aria-labelledby');
  if (labelled.length) return labelled.map(id => text(document.getElementById(id))).join(' ');
  const explicit = node.getAttribute('aria-label');
  if (explicit !== null) return explicit.trim();
  if (node.labels?.length) return [...node.labels].map(text).join(' ');
  return node.tagName === 'BUTTON' ? text(node) : '';
}
function control(node) {
  if (!node) return null;
  return {tag: node.tagName, name: name(node), disabled: node.disabled === true,
    checked: node.checked === true, tabIndex: node.tabIndex,
    brokenReferences: ['aria-labelledby', 'aria-describedby'].flatMap(attribute =>
      referenced(node, attribute).filter(id => !document.getElementById(id)))};
}
const query = selector => root.querySelector(selector);
const visible = node => node.getClientRects().length > 0;
const controls = [...root.querySelectorAll('button,input,select,textarea')].filter(visible);
const confirm = control(query('[data-foreign-plan-confirm]'));
const english = root.getAttribute('aria-label') === 'Foreign history reconstruction review wizard';
return {
  phase: options.phase, expectedGroups: options.expectedGroups,
  wizards: [...document.querySelectorAll('section[aria-label]')]
    .filter(node => node.getAttribute('aria-label') === root.getAttribute('aria-label') && visible(node)).length,
  name: root.getAttribute('aria-label'), form: !!query('[data-foreign-plan-form]'),
  controls: controls.map(control),
  modes: [...root.querySelectorAll('[data-foreign-plan-mode]')].map(node => ({pressed: node.getAttribute('aria-pressed')})),
  groups: [...root.querySelectorAll('[data-foreign-plan-group]')].map(node => ({
    id: node.dataset.foreignPlanGroup, text: text(node),
    namedInput: !!name(node.querySelector('input')),
    missing: node.querySelector('[data-foreign-retained-missing]')?.dataset.foreignRetainedMissing,
    // Report whether context is explicitly linked, without pretending a native
    // input label alone is a complete screen-reader usability assessment.
    linkedContext: !!node.querySelector('input')?.getAttribute('aria-describedby'),
  })),
  review: !!query('[data-foreign-plan-review]'),
  confirm, save: control(query('[data-foreign-plan-save]')),
  preview: control(query('[data-foreign-plan-preview]')),
  execution: control(query('[data-foreign-plan-execution-unavailable]')),
  retry: control(query('[data-foreign-plan-retry]')), status: control(query('[data-foreign-plan-status]')),
  revalidate: control(query('[data-foreign-plan-revalidate]')),
  alert: [...root.querySelectorAll('[role=alert]')].map(text).join(' '),
  savedStatus: text(query('[data-foreign-plan-saved] [role=status]')),
  planOnlyPhrase: english ? 'Save only the system-disk review record' : '仅同意保存系统盘审核记录',
  noImportPhrase: english ? 'this is not an import completion receipt' : '这不是迁入完成回执',
};
