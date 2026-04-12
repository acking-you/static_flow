(function () {
  function isTouchPreferred() {
    try {
      if (window.matchMedia && window.matchMedia("(pointer: coarse)").matches) {
        return true;
      }
    } catch (_) {}
    return window.innerWidth <= 900;
  }

  function readNumber(key, fallbackValue) {
    try {
      var raw = window.localStorage.getItem(key);
      if (!raw) return fallbackValue;
      var parsed = Number(raw);
      return Number.isFinite(parsed) ? parsed : fallbackValue;
    } catch (_) {
      return fallbackValue;
    }
  }

  function writeNumber(key, value) {
    try {
      if (Number.isFinite(value)) {
        window.localStorage.setItem(key, String(value));
      }
    } catch (_) {}
  }

  function removeKey(key) {
    try {
      window.localStorage.removeItem(key);
    } catch (_) {}
  }

  function destroyPlayer(element) {
    if (!element) return;
    var ctx = element.__sfLocalMediaPlayer;
    if (!ctx) return;
    try {
      if (ctx.persist) {
        ctx.persist();
      }
      if (ctx.player && typeof ctx.player.destroy === "function") {
        ctx.player.destroy();
      }
    } catch (_) {}
    element.__sfLocalMediaPlayer = null;
    element.innerHTML = "";
  }

  function supportsNativeHls() {
    try {
      var video = document.createElement("video");
      return !!video.canPlayType("application/vnd.apple.mpegurl");
    } catch (_) {
      return false;
    }
  }

  function applyRestoredState(player, storageKey) {
    var progressKey = storageKey + ":progress";
    var rateKey = storageKey + ":rate";
    var volumeKey = storageKey + ":volume";
    var restoredTime = readNumber(progressKey, 0);
    var restoredRate = readNumber(rateKey, 1);
    var restoredVolume = readNumber(volumeKey, 1);

    var apply = function () {
      try {
        if (restoredRate > 0) {
          player.playbackRate = restoredRate;
        }
        if (restoredVolume >= 0 && restoredVolume <= 1) {
          player.volume = restoredVolume;
        }
        if (restoredTime > 3) {
          player.currentTime = restoredTime;
        }
      } catch (_) {}
    };

    if (typeof player.once === "function") {
      player.once("loadeddata", apply);
      player.once("canplay", apply);
    } else {
      window.setTimeout(apply, 300);
    }
  }

  window.sfLocalMediaPlayerUnmount = function (element) {
    destroyPlayer(element);
  };

  window.sfLocalMediaPlayerMount = function (element, url, mode, title, storageKey) {
    destroyPlayer(element);
    if (!element) {
      throw new Error("Missing mount element");
    }
    if (!window.Player) {
      throw new Error("xgplayer core is not loaded");
    }

    if (mode === "hls" && !supportsNativeHls() && !window.HlsPlayer) {
      throw new Error("xgplayer hls plugin is not loaded");
    }

    var coarse = isTouchPreferred();
    var config = {
      el: element,
      url: url,
      autoplay: false,
      playsinline: true,
      videoInit: true,
      fluid: true,
      width: "100%",
      height: "100%",
      lang: "zh-cn",
      title: title || "",
      presets: window.Player && window.Player.defaultPreset ? [window.Player.defaultPreset] : [],
      isMobileSimulateMode: coarse ? "mobile" : "pc",
      gestureX: true,
      gestureY: true,
      disablePress: false,
      pressRate: 2,
      miniprogress: true,
      pip: false,
      download: false,
      cssFullscreen: false,
      screenShot: false,
      closeVideoClick: false,
      closeVideoDblclick: false,
      playbackRate: [0.75, 1, 1.25, 1.5, 2],
      defaultPlaybackRate: 1
    };

    if (mode === "hls" && !supportsNativeHls()) {
      config.plugins = [window.HlsPlayer];
      config.isLive = false;
      config.hls = config.hls || {};
    }

    var player = new window.Player(config);
    applyRestoredState(player, storageKey);

    var progressKey = storageKey + ":progress";
    var rateKey = storageKey + ":rate";
    var volumeKey = storageKey + ":volume";
    var lastPersistTs = 0;
    var persist = function () {
      var now = Date.now();
      if (now - lastPersistTs < 1200) {
        return;
      }
      lastPersistTs = now;
      try {
        var currentTime = Number(player.currentTime || 0);
        if (Number.isFinite(currentTime) && currentTime > 3) {
          writeNumber(progressKey, currentTime);
        }
        var currentRate = Number(player.playbackRate || 1);
        if (Number.isFinite(currentRate) && currentRate > 0) {
          writeNumber(rateKey, currentRate);
        }
        var currentVolume = Number(player.volume || 0);
        if (Number.isFinite(currentVolume) && currentVolume >= 0 && currentVolume <= 1) {
          writeNumber(volumeKey, currentVolume);
        }
      } catch (_) {}
    };

    if (typeof player.on === "function") {
      player.on("timeupdate", persist);
      player.on("pause", persist);
      player.on("ratechange", persist);
      player.on("volumechange", persist);
      player.on("ended", function () {
        removeKey(progressKey);
      });
    }

    element.__sfLocalMediaPlayer = {
      player: player,
      persist: persist
    };

    return player;
  };
})();
