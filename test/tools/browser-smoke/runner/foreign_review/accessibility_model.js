// Shared assertion model. Its inputs must be collected from the real DOM in
// browser mode; host fixtures prove this checker, not browser accessibility.
return function validateForeignReviewAccessibility(s) {
  const errors = [];
  if (s.wizards !== 1) errors.push('one named review wizard must be visible');
  if (!s.name.trim()) errors.push('review wizard has no accessible name');
  for (const control of s.controls) {
    if (!control.name.trim()) errors.push(`unnamed ${control.tag} control`);
    if (control.brokenReferences.length) errors.push('control has broken ARIA label/description references');
    if (control.tabIndex > 0) errors.push('positive tabindex changes native keyboard order');
  }
  if (s.form) {
    if (s.modes.length !== 3 || s.modes.filter(mode => mode.pressed === 'true').length !== 1
        || s.modes.some(mode => !['true', 'false'].includes(mode.pressed))) {
      errors.push('intent buttons must expose one explicit aria-pressed choice');
    }
    const groups = new Set(s.groups.map(group => group.id));
    if (groups.size !== s.groups.length || groups.size !== s.expectedGroups.length) {
      errors.push('history mapping groups are missing or duplicated');
    }
    for (const expected of s.expectedGroups) {
      const group = s.groups.find(group => group.id === expected.id);
      if (!group || !group.namedInput || !group.text.includes(expected.history)
          || !group.text.includes(expected.origin) || group.missing !== String(expected.missing)) {
        errors.push('history mapping lost its name, provenance or unavailable-body count');
      }
    }
  }
  if (s.review) {
    if (!s.execution || !s.execution.disabled || s.execution.tag !== 'BUTTON') {
      errors.push('execution must remain a disabled native button');
    }
  }
  if (s.phase === 'review') {
    if (!s.confirm || s.confirm.checked || !s.confirm.name.includes(s.planOnlyPhrase)
        || !s.save || !s.save.disabled) {
      errors.push('review needs unchecked plan-only consent before saving');
    }
  }
  if (s.phase === 'unknown') {
    if (!s.alert.trim() || !s.retry || s.retry.disabled || !s.status || s.status.disabled) {
      errors.push('unknown result must announce query and same-request retry');
    }
    if (s.form && (s.controls.some(control => control.tag === 'INPUT' && !control.disabled)
        || !s.preview || !s.preview.disabled)) {
      errors.push('unknown save must fence replacement inputs and review');
    }
  }
  if (s.phase === 'stored') {
    if (!s.savedStatus.includes(s.noImportPhrase) || !s.status || s.status.disabled
        || !s.revalidate || s.revalidate.disabled) {
      errors.push('saved record must announce review-only status and retain recovery controls');
    }
  }
  if (s.phase === 'local' && (!s.preview || !s.preview.disabled || s.review)) {
    errors.push('unsupported local intent must not expose server review or execution');
  }
  return errors;
};
