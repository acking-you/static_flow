import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import test from 'node:test';

const bridgePath = path.resolve('frontend/static/local_media_player_bridge.js');
const bridgeSource = fs.readFileSync(bridgePath, 'utf8');

function createEnvironment({ nativeHls = false } = {}) {
  const mounts = [];
  const storage = new Map();

  function Player(config) {
    this.config = config;
    this.playbackRate = 1;
    this.volume = 1;
    this.destroy = () => {};
    this.on = () => {};
    this.once = () => {};
    mounts.push({ ctor: 'Player', config, instance: this });
  }
  Player.defaultPreset = { name: 'default-preset' };

  function HlsPlayerPlugin() {}
  HlsPlayerPlugin.pluginName = 'hls';
  HlsPlayerPlugin.isSupported = () => true;

  function createNode(tag = 'div') {
    return {
      tagName: tag.toUpperCase(),
      style: {},
      children: [],
      parentNode: null,
      textContent: '',
      innerHTML: '',
      listeners: new Map(),
      appendChild(child) {
        child.parentNode = this;
        this.children.push(child);
        return child;
      },
      removeChild(child) {
        this.children = this.children.filter((value) => value !== child);
        if (child) {
          child.parentNode = null;
        }
      },
      addEventListener(type, handler) {
        const current = this.listeners.get(type) || [];
        current.push(handler);
        this.listeners.set(type, current);
      },
      removeEventListener(type, handler) {
        const current = this.listeners.get(type) || [];
        this.listeners.set(
          type,
          current.filter((value) => value !== handler),
        );
      },
      dispatch(type, event = {}) {
        const handlers = this.listeners.get(type) || [];
        handlers.forEach((handler) => handler(event));
      },
      setAttribute(name, value) {
        this[name] = value;
      },
    };
  }

  const window = {
    Player,
    HlsPlayer: HlsPlayerPlugin,
    innerWidth: 390,
    matchMedia: () => ({ matches: true }),
    localStorage: {
      getItem(key) {
        return storage.has(key) ? storage.get(key) : null;
      },
      setItem(key, value) {
        storage.set(key, String(value));
      },
      removeItem(key) {
        storage.delete(key);
      },
    },
    setTimeout(fn) {
      fn();
      return 1;
    },
    clearTimeout() {},
  };

  const document = {
    createElement(tag) {
      if (tag === 'video') {
        return {
          canPlayType(mime) {
            return nativeHls && mime === 'application/vnd.apple.mpegurl' ? 'probably' : '';
          },
        };
      }
      return createNode(tag);
    },
  };

  const context = vm.createContext({
    window,
    document,
    console,
    Number,
    Date,
  });
  vm.runInContext(bridgeSource, context, { filename: bridgePath });

  const element = createNode('div');
  element.__sfLocalMediaPlayer = null;

  return { window, mounts, element };
}

test('hls mode uses Player with HlsPlayer plugin instead of constructing the plugin directly', () => {
  const { window, mounts, element } = createEnvironment({ nativeHls: false });

  window.sfLocalMediaPlayerMount(
    element,
    '/admin/local-media/api/playback/hls/demo/index.m3u8',
    'hls',
    'Demo',
    'sf-local-media-progress:demo',
  );

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0].ctor, 'Player');
  assert.equal(mounts[0].config.plugins.length, 1);
  assert.equal(mounts[0].config.plugins[0], window.HlsPlayer);
  assert.equal(mounts[0].config.url, '/admin/local-media/api/playback/hls/demo/index.m3u8');
  assert.equal(mounts[0].config.isLive, false);
});

test('player bridge uses Player.defaultPreset when available', () => {
  const { mounts, element, window } = createEnvironment({ nativeHls: false });

  window.sfLocalMediaPlayerMount(
    element,
    '/admin/local-media/api/playback/raw?file=demo.mp4',
    'raw',
    'Demo',
    'sf-local-media-progress:demo',
  );

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0].config.presets.length, 1);
  assert.equal(mounts[0].config.presets[0], window.Player.defaultPreset);
});

test('coarse-pointer long press switches to 2x and shows centered badge until release', () => {
  const { mounts, element, window } = createEnvironment({ nativeHls: false });

  window.sfLocalMediaPlayerMount(
    element,
    '/admin/local-media/api/playback/raw?file=demo.mp4',
    'raw',
    'Demo',
    'sf-local-media-progress:demo',
  );

  const player = mounts[0].instance;
  assert.equal(player.playbackRate, 1);

  element.dispatch('touchstart', {
    touches: [{ clientX: 32, clientY: 48 }],
  });

  assert.equal(player.playbackRate, 2);
  assert.equal(element.children.at(-1).textContent, '2x');
  assert.equal(element.children.at(-1).style.opacity, '1');

  element.dispatch('touchend', {});

  assert.equal(player.playbackRate, 1);
  assert.equal(element.children.at(-1).style.opacity, '0');
});
