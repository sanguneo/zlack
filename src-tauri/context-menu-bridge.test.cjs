const test = require('node:test');
const assert = require('node:assert/strict');
const { createPointerHandler } = require('./context-menu-bridge.cjs');

function menuTarget(button, menuSelector) {
  return {
    closest(selector) {
      if (selector === `${menuSelector} button`) return button;
      if (selector === menuSelector) return button ? {} : null;
      return null;
    },
  };
}

test('pointerdown runs a menu action before page handlers', () => {
  const menuSelector = '#zlack-image-context-menu';
  const button = {};
  const actions = new WeakMap();
  const order = [];
  actions.set(button, () => order.push('action'));
  const handler = createPointerHandler({
    actionByButton: actions,
    dismiss: () => order.push('dismiss'),
    isOpen: () => true,
    menuSelector,
  });
  const event = {
    type: 'pointerdown',
    target: menuTarget(button, menuSelector),
    preventDefault: () => order.push('prevent'),
    stopImmediatePropagation: () => order.push('stop'),
  };

  assert.equal(handler(event), true);
  assert.deepEqual(order, ['prevent', 'stop', 'dismiss', 'action']);
  assert.equal(handler(event), false);
  assert.deepEqual(order, ['prevent', 'stop', 'dismiss', 'action']);
});

test('outside pointerdown dismisses an open menu', () => {
  let dismissed = 0;
  const handler = createPointerHandler({
    actionByButton: new WeakMap(),
    dismiss: () => { dismissed += 1; },
    isOpen: () => true,
    menuSelector: '#zlack-image-context-menu',
  });

  handler({
    type: 'pointerdown',
    target: menuTarget(null, '#zlack-image-context-menu'),
  });

  assert.equal(dismissed, 1);
});

test('each image menu command runs through the shared pointer handler', () => {
  const menuSelector = '#zlack-image-context-menu';
  const actions = new WeakMap();
  const invoked = [];
  const buttons = ['Save', 'Copy', 'Downloads'].map((label) => {
    const button = {};
    actions.set(button, () => invoked.push(label));
    return button;
  });
  const handler = createPointerHandler({
    actionByButton: actions,
    dismiss: () => {},
    isOpen: () => true,
    menuSelector,
  });

  for (const button of buttons) {
    handler({
      type: 'pointerdown',
      target: menuTarget(button, menuSelector),
      preventDefault: () => {},
      stopImmediatePropagation: () => {},
    });
  }

  assert.deepEqual(invoked, ['Save', 'Copy', 'Downloads']);
});

test('keyboard click runs the same menu action path', () => {
  const menuSelector = '#zlack-image-context-menu';
  const button = {};
  const actions = new WeakMap();
  let invoked = 0;
  actions.set(button, () => { invoked += 1; });
  const handler = createPointerHandler({
    actionByButton: actions,
    dismiss: () => {},
    isOpen: () => true,
    menuSelector,
  });

  assert.equal(handler({
    type: 'click',
    target: menuTarget(button, menuSelector),
    preventDefault: () => {},
    stopImmediatePropagation: () => {},
  }), true);
  assert.equal(invoked, 1);
});
