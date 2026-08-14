const ZlackContextMenuBridge = (function buildContextMenuBridge() {
  function createPointerHandler(options) {
    const { actionByButton, dismiss, isOpen, menuSelector } = options;

    return function handleMenuPointer(event) {
      const target = event.target;
      const button = target?.closest?.(`${menuSelector} button`);
      const action = button && actionByButton.get(button);

      if (action) {
        event.preventDefault();
        event.stopImmediatePropagation();
        actionByButton.delete(button);
        dismiss();
        action();
        return true;
      }

      if (isOpen() && !target?.closest?.(menuSelector)) {
        dismiss();
      }
      return false;
    };
  }

  return { createPointerHandler };
})();

if (typeof module !== 'undefined' && module.exports) {
  module.exports = ZlackContextMenuBridge;
}
