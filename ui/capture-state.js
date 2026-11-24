(() => {
  "use strict";

  function isCaptureSuspended(captureState) {
    return Boolean(
      captureState?.paused ||
      captureState?.privacyMode ||
      captureState?.privacy_mode,
    );
  }

  const api = Object.freeze({ isCaptureSuspended });
  if (typeof module !== "undefined" && module.exports) {
    module.exports = api;
  } else {
    window.MiaoClipCaptureState = api;
  }
})();
