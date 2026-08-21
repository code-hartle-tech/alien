/* alien.hartle.tech — behaviour and sound.
 *
 * No dependencies, no build step, no network requests. That is not minimalism
 * for its own sake: the page ships inside a Caddy container with a strict CSP,
 * and every byte it needs has to be in the image.
 *
 * ── What makes noise here ───────────────────────────────────────────────────
 * The background music is a licensed track, served from this origin and played
 * through the Web Audio graph so it shares the limiter and the room with
 * everything else. Credit is in the page footer and in CREDITS.md.
 *
 * Everything else — the motion-tracker ping, the wet squelch, the servo click,
 * the confirmation tones, the radio squelch — is generated at runtime from
 * oscillators and noise. The announcer's sixteen lines are pre-rendered with
 * espeak-ng and processed offline, because browser speech synthesis cannot be
 * routed through Web Audio and so can never be filtered or reverberated.
 *
 * There is no synthesised background bed any more; the track replaced it.
 */

(() => {
  'use strict';

  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* ── Audio ───────────────────────────────────────────────────────────────
     One lazily-created AudioContext. Browsers refuse to start one before a
     user gesture, so it is built on the first toggle click and never before —
     which conveniently means muted-by-default is enforced by the platform as
     well as by us. */

  const Sound = (() => {
    let ctx = null;
    let master = null;
    /* The remembered preference. This was briefly forced to `false` after a
       bug where the button rendered "off" while the flag was `true`, which
       inverted the toggle for anyone who had enabled sound before.

       The real defect there was that the BUTTON was painted unconditionally
       rather than from this flag. It is painted from the flag now, so the two
       cannot disagree, and the preference can be honoured properly: someone
       who turned audio on last visit gets it again, as soon as the platform
       permits. */
    let on = (() => {
      try { return localStorage.getItem('alien-sound') === 'on'; } catch { return false; }
    })();

    let sfx = null;   // muffled bus for every effect except the copy sound
    let wet = null;   // shared reverb send — the "damp" in damp and gruesome

    function ensure() {
      if (ctx) return ctx;
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();

      // A limiter on the way out, and it is not optional. Five layers plus a
      // reverb tail plus interaction sounds sum well past 1.0 on peaks, and
      // Web Audio does not clip gracefully — it crackles. A compressor with a
      // hard knee and a fast attack turns "crackling" into "loud", which is
      // the difference between broken and mixed.
      const limiter = ctx.createDynamicsCompressor();
      limiter.threshold.value = -8;
      limiter.knee.value = 0;
      limiter.ratio.value = 20;
      limiter.attack.value = 0.003;
      limiter.release.value = 0.25;
      limiter.connect(ctx.destination);

      master = ctx.createGain();
      master.gain.value = 0.0;
      master.connect(limiter);

      // One convolver for the whole page. Every layer and every interaction
      // sound sends into it, which is what makes them sound like they are in
      // the same wet concrete room rather than in a browser.
      const verb = ctx.createConvolver();
      verb.buffer = impulse(2.6, 2.4);
      const verbGain = ctx.createGain();
      verbGain.gain.value = 0.3;
      verb.connect(verbGain).connect(master);

      wet = ctx.createGain();
      wet.gain.value = 0.6;
      wet.connect(verb);

      /* ── Effects bus ──────────────────────────────────────────────────
         Everything except the copy confirmation goes through here: one gain
         to hold the whole ambient palette down, then the same "closed door"
         treatment the soundtrack gets.

         The cutoff is a little higher than the music's 1500 Hz and the shelf a
         little gentler, which is what puts these just ABOVE the track rather
         than buried underneath it — same room, same door, standing slightly
         nearer to it. This pair moves WITH the music's door: the effect the
         brief asks for is the gap between them, so if one cutoff changes the
         other has to follow or they collapse onto each other.

         The copy sound deliberately does NOT come through here. It is direct
         feedback for something the visitor just did, and muffling it would
         make the button feel broken. */
      const sfxDoor = ctx.createBiquadFilter();
      sfxDoor.type = 'lowpass';
      sfxDoor.frequency.value = 2200;
      sfxDoor.Q.value = 0.6;

      const sfxWall = ctx.createBiquadFilter();
      sfxWall.type = 'highshelf';
      sfxWall.frequency.value = 4400;
      sfxWall.gain.value = -6;

      sfx = ctx.createGain();
      sfx.gain.value = 0.22;          // drastically down from unity
      sfx.connect(sfxDoor).connect(sfxWall).connect(master);

      const sfxSend = ctx.createGain();
      sfxSend.gain.value = 0.5;       // wetter than the track: further away
      sfx.connect(sfxSend).connect(wet);

      /* The context can come up at a moment nothing else is watching for — a
         gesture handled elsewhere, an autoplay policy that relents, a device
         change. This is the single place that notices, and it is what moves the
         track off the native path and into the graph at exactly the right
         instant: not before (silence) and not seconds later (no door). */
      ctx.addEventListener('statechange', () => {
        if (ctx.state !== 'running' || !on) return;
        ensureAudible();
        startTrack();
        if (stateWatcher) stateWatcher();
      });

      return ctx;
    }

    function noiseBuffer(seconds) {
      const n = Math.floor(ctx.sampleRate * seconds);
      const buf = ctx.createBuffer(1, n, ctx.sampleRate);
      const d = buf.getChannelData(0);
      for (let i = 0; i < n; i++) d[i] = Math.random() * 2 - 1;
      return buf;
    }

    /* A synthetic impulse response: stereo noise under an exponential decay,
       with the two channels decorrelated so the tail has width. Cheap, and it
       beats shipping a real IR file we would have to licence. */
    function impulse(seconds, decay) {
      const n = Math.floor(ctx.sampleRate * seconds);
      const buf = ctx.createBuffer(2, n, ctx.sampleRate);
      for (let c = 0; c < 2; c++) {
        const d = buf.getChannelData(c);
        for (let i = 0; i < n; i++) {
          // A short noisy pre-delay before the tail reads as a bigger space.
          const t = i / n;
          d[i] = (Math.random() * 2 - 1) * Math.pow(1 - t, decay);
        }
      }
      return buf;
    }

    const rand = (a, b) => a + Math.random() * (b - a);

    /* Motion tracker. Deliberately NOT a clean tone: a pure sine at these
       frequencies is a doorbell. A sawtooth through a narrow bandpass, with a
       noise transient on the front, lands as a sensor return instead — the
       grit is doing the work, not the pitch. */
    function ping() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      [[0, 0.34], [0.13, 0.12]].forEach(([delay, level]) => {
        const o = ctx.createOscillator();
        const g = ctx.createGain();
        const bp = ctx.createBiquadFilter();
        bp.type = 'bandpass';
        bp.Q.value = 6;
        bp.frequency.setValueAtTime(1150, t + delay);
        bp.frequency.exponentialRampToValueAtTime(520, t + delay + 0.11);
        o.type = 'sawtooth';
        o.frequency.setValueAtTime(430, t + delay);
        o.frequency.exponentialRampToValueAtTime(210, t + delay + 0.1);
        g.gain.setValueAtTime(0.0001, t + delay);
        g.gain.exponentialRampToValueAtTime(level, t + delay + 0.005);
        g.gain.exponentialRampToValueAtTime(0.0001, t + delay + 0.15);
        o.connect(bp).connect(g);
        g.connect(sfx);
        g.connect(wet);
        o.start(t + delay);
        o.stop(t + delay + 0.2);
      });

      // Transient: the click of the sensor keying, not part of the tone.
      const src = ctx.createBufferSource();
      src.buffer = noiseBuffer(0.05);
      const hp = ctx.createBiquadFilter();
      hp.type = 'highpass';
      hp.frequency.value = 1800;
      const ng = ctx.createGain();
      ng.gain.setValueAtTime(0.1, t);
      ng.gain.exponentialRampToValueAtTime(0.0001, t + 0.04);
      src.connect(hp).connect(ng);
      ng.connect(sfx);
      src.start(t);
      src.stop(t + 0.06);
    }

    /* Wet: a noise burst pushed through a lowpass that collapses fast, with a
       little resonance. The falling cutoff is the whole trick — it is what the
       ear hears as something viscous closing. */
    function squelch() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const src = ctx.createBufferSource();
      src.buffer = noiseBuffer(0.4);

      const lp = ctx.createBiquadFilter();
      lp.type = 'lowpass';
      lp.Q.value = 12;
      lp.frequency.setValueAtTime(2400, t);
      lp.frequency.exponentialRampToValueAtTime(160, t + 0.28);

      const g = ctx.createGain();
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(0.22, t + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 0.34);

      src.connect(lp).connect(g);
      g.connect(sfx);
      g.connect(wet);
      src.start(t);
      src.stop(t + 0.4);
    }

    /* Mechanical: a very short sawtooth chirp. Servos and relays. */
    function servo() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      const hp = ctx.createBiquadFilter();
      hp.type = 'highpass';
      hp.frequency.value = 400;
      o.type = 'sawtooth';
      o.frequency.setValueAtTime(180, t);
      o.frequency.exponentialRampToValueAtTime(90, t + 0.05);
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(0.13, t + 0.004);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 0.07);
      o.connect(hp).connect(g);
      g.connect(sfx);
      g.connect(wet);
      o.start(t);
      o.stop(t + 0.09);
    }

    /* Confirmation: two rising triangle tones. Brighter and more musical than
       everything else here, and that is the point — it is the one sound that
       fires when something actually worked, so it should not sound like the
       ship. Briefly replaced with a mechanical latch; the latch was duller and
       read as another failure rather than a success. */
    function confirm() {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      [660, 990].forEach((f, i) => {
        const o = ctx.createOscillator();
        const g = ctx.createGain();
        o.type = 'triangle';
        o.frequency.value = f;
        g.gain.setValueAtTime(0.0001, t + i * 0.07);
        g.gain.exponentialRampToValueAtTime(0.16, t + i * 0.07 + 0.01);
        g.gain.exponentialRampToValueAtTime(0.0001, t + i * 0.07 + 0.18);
        o.connect(g);
        g.connect(master);
        g.connect(wet);
        o.start(t + i * 0.07);
        o.stop(t + i * 0.07 + 0.2);
      });
    }

    /* ── Radio ────────────────────────────────────────────────────────────
       Comms chatter and a ship announcer, spoken by the browser's own speech
       synthesiser. No dialogue is sampled from anything — the lines below are
       written here, and the voice is whatever the visitor's system provides,
       pitched down and slowed so it reads as a machine rather than an
       assistant.

       The squelch bursts either side are what actually sell it: a radio is
       recognised by its keying, not by the voice. Those go through the Web
       Audio graph and therefore through the limiter and the room; the speech
       itself cannot (there is no capture path for it), so its volume is set
       conservatively instead. */

    function squelchBurst(len, level) {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const src = ctx.createBufferSource();
      src.buffer = noiseBuffer(len + 0.05);
      const bp = ctx.createBiquadFilter();
      bp.type = 'bandpass';
      bp.frequency.value = 1600;
      bp.Q.value = 0.9;
      const g = ctx.createGain();
      g.gain.setValueAtTime(0.0001, t);
      g.gain.exponentialRampToValueAtTime(level, t + 0.008);
      g.gain.exponentialRampToValueAtTime(0.0001, t + len);
      src.connect(bp).connect(g);
      g.connect(sfx);
      g.connect(wet);
      src.start(t);
      src.stop(t + len + 0.05);
    }

    // Static tear, used for the picture faults as well as the radio.
    function statics(len) {
      if (!on || !ctx) return;
      const t = ctx.currentTime;
      const src = ctx.createBufferSource();
      src.buffer = noiseBuffer(len + 0.1);
      const hp = ctx.createBiquadFilter();
      hp.type = 'highpass';
      hp.frequency.value = 900;
      const g = ctx.createGain();
      g.gain.setValueAtTime(0.14, t);
      g.gain.exponentialRampToValueAtTime(0.0001, t + len);
      src.connect(hp).connect(g);
      g.connect(sfx);
      src.start(t);
      src.stop(t + len + 0.1);
    }

    /* The announcer lines are pre-rendered, not spoken by the browser.
       speechSynthesis output cannot be routed through Web Audio — there is no
       capture path — so it can never be filtered, driven or reverberated, and
       it always ends up sounding like a screen reader. These clips were
       rendered with espeak-ng and processed offline through a radio passband,
       broadcast compression, two short echo taps and a slow phaser, so they
       arrive already sounding like comms. Playing them as buffers also puts
       them through the same limiter and the same room as everything else.

       Sixteen clips, 120 KB total, fetched once on first use and cached. */

    const VOICE_COUNT = 16;
    const voiceBuffers = new Map();
    let voiceLoading = false;

    async function loadVoices() {
      if (voiceLoading || voiceBuffers.size) return;
      voiceLoading = true;
      const jobs = [];
      for (let i = 1; i <= VOICE_COUNT; i++) {
        const id = String(i).padStart(2, '0');
        jobs.push(
          fetch(`audio/voice/${id}.opus`)
            .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(String(r.status)))))
            .then((b) => ctx.decodeAudioData(b))
            .then((buf) => voiceBuffers.set(id, buf))
            .catch(() => { /* one missing clip must not take the rest down */ })
        );
      }
      await Promise.all(jobs);
      voiceLoading = false;
    }

    function say() {
      if (!on || !ctx) return;
      if (!voiceBuffers.size) { loadVoices(); return; }

      const keys = [...voiceBuffers.keys()];
      const buf = voiceBuffers.get(keys[Math.floor(Math.random() * keys.length)]);
      if (!buf) return;

      // Key the transmitter, speak, unkey. The squelch either side is what the
      // ear actually uses to identify a radio.
      squelchBurst(0.09, 0.1);
      const t = ctx.currentTime + 0.14;

      const src = ctx.createBufferSource();
      src.buffer = buf;
      // Small pitch variance so repeats of the same line are not identical.
      src.playbackRate.value = 0.97 + Math.random() * 0.06;

      const g = ctx.createGain();
      g.gain.value = 0.62;
      src.connect(g);
      g.connect(sfx);
      // Heavier reverb send than the interaction sounds: a voice over a PA in a
      // steel corridor is mostly room.
      const send = ctx.createGain();
      send.gain.value = 0.5;
      g.connect(send).connect(wet);

      src.start(t);
      src.onended = () => { if (on) squelchBurst(0.07, 0.07); };
    }

    function hush() { /* buffers stop with the graph; nothing to cancel */ }

    /* ── The bed ──────────────────────────────────────────────────────────
       A real track, not a loop generated here: "They Come at Night" by
       morgantj, CC BY 3.0, from ccMixter. Credited in the page footer and in
       CREDITS.md, which is what CC BY asks for. It is routed through the same
       limiter and the same reverb send as everything else, so the interaction
       sounds sit in the track rather than on top of it.

       The synthesised bed below is the fallback. If the file 404s, is blocked,
       or the browser refuses both codecs, the page still has an atmosphere
       instead of silence. */

    // Set by the UI so the toggle can repaint the moment audio genuinely
    // starts, instead of waiting for the next poll to notice.
    let stateWatcher = null;

    let track = null;
    // createMediaElementSource may be called only ONCE per element — a second
    // call throws, and the whole toggle would fall back to the synth bed
    // permanently after the first off/on cycle. Build it once, reuse it.
    let trackSource = null;
    let trackGain = null;

    function startTrack() {
      const audioEl = document.getElementById('bed');
      if (!audioEl) return false;

      /* ── Do not route a suspended graph ───────────────────────────────────
         createMediaElementSource permanently takes the element off the default
         audio output and sends it into the graph instead. If the graph is
         suspended when that happens, the element goes silent and STAYS silent:
         it reports playing, its currentTime advances, and nothing comes out.

         That is worse than doing nothing, because a media element and an
         AudioContext are governed by two different autoplay policies. The
         element alone will often be allowed to play on load; the context needs
         a gesture. Routing early therefore converts audio that would have been
         audible into audio that cannot be.

         So until the context is genuinely running, leave the element alone and
         let it play natively — undoored, but audible. The statechange handler
         in ensure() routes it the instant the context comes up. */
      if (!ctx || ctx.state !== 'running') {
        audioEl.volume = 1;
        const p = audioEl.play();
        if (p && p.catch) p.catch(() => { /* autoplay refused; a gesture will do it */ });
        return false;
      }

      try {
        if (!trackSource) {
          trackSource = ctx.createMediaElementSource(audioEl);

          // ── "through a closed door, one room over" ──────────────────────
          // A wall is a lowpass. High frequencies lose their energy in the
          // material and the bass walks straight through, which is why muffled
          // music from another room is all thump and no detail.
          //
          // The cutoff was 620 Hz, and that was too far. Measured on the actual
          // track: the filtered signal holds its overall level (-23.5 dB, all
          // but unchanged), but once you high-pass at 200 Hz to model what a
          // laptop or phone speaker can physically move, it collapses to
          // -34.9 dB. Everything the door left behind was living underneath the
          // speaker's own roll-off. On headphones it was thump; on the hardware
          // most visitors have, it was silence — which is exactly how it was
          // reported.
          //
          // 1500 Hz still reads as "one room over, door shut" — the detail and
          // air are gone — but it leaves midrange the speaker can actually
          // reproduce, recovering about 5 dB of that.
          const door = ctx.createBiquadFilter();
          door.type = 'lowpass';
          door.frequency.value = 1500;
          door.Q.value = 0.7;

          const wall = ctx.createBiquadFilter();
          wall.type = 'highshelf';
          wall.frequency.value = 3000;
          wall.gain.value = -8;

          trackGain = ctx.createGain();
          trackGain.gain.value = 0.0001;
          trackSource.connect(door).connect(wall).connect(trackGain);
          trackGain.connect(master);

          // Heavier reverb than the interaction sounds get. The corridor
          // between you and the source is most of what you are hearing.
          const send = ctx.createGain();
          send.gain.value = 0.4;
          trackGain.connect(send).connect(wet);
        }

        const t = ctx.currentTime;
        trackGain.gain.cancelScheduledValues(t);
        trackGain.gain.setValueAtTime(Math.max(trackGain.gain.value, 0.0001), t);
        // The file is mastered to -22 LUFS, which is deliberately quiet, and the
        // door above takes more off it. Unity here is not enough to hear; the
        // limiter on the output is what makes this safe to push.
        trackGain.gain.exponentialRampToValueAtTime(2.1, t + 3);

        audioEl.volume = 1;
        const p = audioEl.play();
        if (p && p.catch) {
          // Autoplay refused, or the browser took neither codec. Nothing to
          // fall back to now that the synthesised bed is gone — the
          // interaction sounds and the announcer still work.
          p.catch(() => { track = null; });
        }
        track = { audioEl };
        return true;
      } catch {
        return false;
      }
    }

    function stopTrack() {
      if (!track || !trackGain) return;
      const t = ctx.currentTime;
      trackGain.gain.cancelScheduledValues(t);
      trackGain.gain.setValueAtTime(Math.max(trackGain.gain.value, 0.0001), t);
      trackGain.gain.exponentialRampToValueAtTime(0.0001, t + 0.7);
      const dying = track;
      track = null;
      setTimeout(() => { try { dying.audioEl.pause(); } catch { /* gone */ } }, 800);
    }

    /* Idempotent recovery. Safe to call as often as you like: it does nothing
       unless audio is wanted, and otherwise nudges whatever is broken back
       into place — a suspended context, a track that was never started, or one
       that got paused by something outside our control.

       This is the difference between "we tried once at load" and "it plays".
       A page can lose audio for a dozen reasons that are not the user's doing:
       autoplay policy on a fresh load, a bfcache restore that comes back
       paused, a tab that was backgrounded, an OS audio-device change. Trying
       once at startup handles none of them. */
    /* master starts at 0 and only setOn() ever ramped it up. A visitor whose
       stored preference is already "on" never goes through setOn() — the boot
       path calls keepAlive() instead — so the graph came up, the track was
       routed into it, and every last decibel was multiplied by zero. The toggle
       said "on" because the toggle was on. Raising it here is what makes the
       restored preference mean anything. */
    function ensureAudible() {
      if (!ctx || !master) return;
      if (master.gain.value >= 0.85) return;
      const t = ctx.currentTime;
      master.gain.cancelScheduledValues(t);
      master.gain.setValueAtTime(master.gain.value, t);
      master.gain.linearRampToValueAtTime(0.9, t + 0.4);
    }

    function keepAlive() {
      if (!on) return;
      ensure();
      if (!ctx) return;
      if (ctx.state === 'suspended') ctx.resume().catch(() => {});
      ensureAudible();
      const el = document.getElementById('bed');
      if (!el) return;
      // Not running yet: keep it on the native path rather than routing it into
      // a graph that cannot pass audio. startTrack() enforces this too.
      if (ctx.state !== 'running') { startTrack(); return; }
      if (!trackSource) { startTrack(); return; }
      if (el.paused) el.play().catch(() => {});
    }

    function setOn(next) {
      on = next;
      try { localStorage.setItem('alien-sound', next ? 'on' : 'off'); } catch { /* private mode */ }
      if (!on) {
        // Tear it down rather than only muting: left running behind a zero
        // gain it keeps a decoder, oscillators and timers alive forever, which
        // is rude on a laptop battery.
        hush();
        stopTrack();
        if (master) master.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.3);
        return;
      }
      ensure();
      if (!ctx) return;
      // resume() rejects when the gesture requirement is not met, and an
      // unhandled rejection here is a console error on a page that is working
      // as designed. The statechange handler picks it up if it later succeeds.
      if (ctx.state === 'suspended') ctx.resume().catch(() => {});
      master.gain.cancelScheduledValues(ctx.currentTime);
      master.gain.linearRampToValueAtTime(0.9, ctx.currentTime + 0.4);
      startTrack();
    }

    /* Is the soundtrack actually audible this instant — not merely requested?
       `on` only says the toggle is set; the track can still be blocked by
       autoplay policy, stalled, or have failed both codecs. */
    /* `ctx.state === 'running'` is part of the question, not a detail. A routed
       element on a suspended graph advances its currentTime and reports itself
       playing while emitting nothing — so without this clause the check returns
       true during exactly the failure it exists to detect, and the ambient
       layer then chirps away over what the visitor hears as silence. */
    const trackPlaying = () =>
      on && !!ctx && ctx.state === 'running' &&
      !!track && !!track.audioEl && !track.audioEl.paused;

    /* Background palette.
       Same sounds, but they refuse to fire unless the soundtrack is playing,
       so the ambient layer can never chirp away on its own over silence. The
       scenes, the display faults, the announcer and the passer-by all use
       these; anything the visitor directly triggered does not, because
       feedback for a click should not depend on whether music happens to be
       running. */
    const bg = {
      ping:    () => { if (trackPlaying()) ping(); },
      squelch: () => { if (trackPlaying()) squelch(); },
      servo:   () => { if (trackPlaying()) servo(); },
      statics: (n) => { if (trackPlaying()) statics(n); },
      say:     () => { if (trackPlaying()) say(); },
    };

    return {
      ping, squelch, servo, confirm, statics, say, setOn, bg, keepAlive,
      isOn: () => on,
      isTrackPlaying: trackPlaying,
      // Whether audio is genuinely running, as opposed to merely requested:
      // a suspended context means the gesture requirement is still unmet.
      isRunning: () => !!ctx && ctx.state === 'running' && on,
      // Audible right now by any route — including the native path taken before
      // the context is up, where there is no graph to inspect.
      isAudible: () => {
        if (!on) return false;
        const el = document.getElementById('bed');
        if (!el || el.paused || el.muted) return false;
        return trackSource ? (!!ctx && ctx.state === 'running') : true;
      },
      setStateWatcher: (fn) => { stateWatcher = fn; },
    };
  })();

  /* ── Sound toggle ─────────────────────────────────────────────────────── */

  const soundBtn = document.getElementById('sound');

  /* The button reports what the visitor can HEAR, not what the setting says.
     Those come apart on every first load: the stored preference is restored
     synchronously, but nothing may play until a gesture unblocks it, and a
     control claiming "audio on" over silence reads as a broken page rather
     than as a browser policy. The third state says whose move it is. */
  function paintSound() {
    const wanted = Sound.isOn();
    const audible = Sound.isAudible();
    const label = !wanted ? '○ audio off'
                : audible ? '● audio on'
                : '◐ tap for audio';
    soundBtn.setAttribute('aria-pressed', String(wanted));
    soundBtn.classList.toggle('pending', wanted && !audible);
    // Only touch the DOM when it actually changed: this runs on a poll.
    if (soundBtn.textContent !== label) soundBtn.textContent = label;
  }
  Sound.setStateWatcher(paintSound);

  soundBtn.addEventListener('click', () => {
    Sound.setOn(!Sound.isOn());
    paintSound();
    if (Sound.isOn()) Sound.ping();
  });

  /* ── Keep it playing, whatever happens ───────────────────────────────────
     The music used to stop and not come back: sometimes on reload, sometimes
     after following a link and returning. That is not one bug, it is a family
     of them, and trying once at startup handles none:

       · a fresh load has no user gesture, so autoplay is refused outright;
       · returning via back/forward restores from bfcache, often paused, and
         fires no load event at all — only `pageshow`;
       · a backgrounded tab may have its audio suspended by the browser;
       · an OS audio-device change can pause the element from underneath us.

     So instead of one attempt there is one idempotent recovery function and a
     lot of things that call it. `Sound.keepAlive()` does nothing unless audio
     is wanted, so calling it liberally is free.

     The gesture listeners stay armed until audio is genuinely audible, rather
     than being torn down after the first optimistic try. */

  const resume = () => Sound.keepAlive();

  const GESTURES = ['pointerdown', 'keydown', 'touchstart', 'click'];
  function armGestures() {
    GESTURES.forEach((e) => window.addEventListener(e, onGesture, { capture: true, passive: true }));
  }
  function onGesture(e) {
    // Never act on a click of the toggle itself. This listener is on the
    // capture phase, so it runs BEFORE the button's own handler: turning audio
    // on here and letting the button immediately toggle it back off made the
    // control look dead. The button owns its own clicks.
    if (e && e.target && soundBtn.contains(e.target)) return;

    // An explicit "off" is a decision, not a problem to be repaired. Only a
    // visitor who has never chosen, or who chose "on", gets started by a
    // gesture — otherwise muting the page would last exactly until the next
    // click anywhere.
    let stored = null;
    try { stored = localStorage.getItem('alien-sound'); } catch { /* private mode */ }
    if (stored === 'off') return;

    // A gesture is the one thing that unblocks a suspended AudioContext, so
    // this is where a refused autoplay finally succeeds.
    if (!Sound.isOn()) Sound.setOn(true);
    Sound.keepAlive();
    paintSound();
    // Stand down only once the graph is genuinely running. The old check was
    // isTrackPlaying(), which was true for a routed-but-suspended element —
    // so the listeners tore themselves down at the one moment a gesture was
    // still the only thing that could ever have fixed it.
    if (Sound.isRunning()) {
      GESTURES.forEach((ev) => window.removeEventListener(ev, onGesture, true));
    }
  }
  armGestures();

  // Returning from another page. `persisted` means it came out of bfcache, in
  // which case no other lifecycle event will fire.
  window.addEventListener('pageshow', resume);
  // Coming back to the tab.
  document.addEventListener('visibilitychange', () => { if (!document.hidden) resume(); });
  window.addEventListener('focus', resume);

  // The element itself pausing when nobody asked it to.
  const bedEl = document.getElementById('bed');
  if (bedEl) {
    bedEl.addEventListener('pause', () => { if (Sound.isOn()) setTimeout(resume, 250); });
    // `loop` should make this unreachable; it is here because "should" is not
    // a guarantee and silence is the failure we are trying to eliminate.
    bedEl.addEventListener('ended', resume);
    bedEl.addEventListener('stalled', () => setTimeout(resume, 1000));
  }

  // Last line of defence. Cheap — it returns immediately unless audio is
  // wanted and something is actually wrong. The repaint rides along so the
  // label can never sit lying about a state that has since changed.
  setInterval(() => { resume(); paintSound(); }, 4000);

  // And try right now, in case the platform is feeling generous (Chrome grants
  // autoplay on domains where media has been played before).
  paintSound();
  resume();

  /* ── Boot sequence ────────────────────────────────────────────────────── */

  const boot = document.getElementById('boot');
  const bootOut = document.getElementById('boot-out');
  const bootFill = document.getElementById('boot-fill');
  const bootPct = document.getElementById('boot-pct');

  const BOOT_LINES = [
    'HARTLE.TECH INTERFACE // acer gaming-wmi',
    '',
    'probing  /proc/acpi/call ................ present',
    'probing  WMBH  0x79772EC5-04B1-4bfd-843C-61E7F77B6CC9 .. ok',
    'probing  WMAA  ACER_WMID .................. ok',
    'sensors  cpu / gpu / board ............... 3 online',
    'fans     cpu 5882 rpm · gpu 6122 rpm ..... nominal',
    'lighting 4 zones ........................ addressable',
    'firmware V2.04 .......................... unlocked',
    '',
    'no vendor software detected. good.',
  ];

  /* ── Preload ──────────────────────────────────────────────────────────────
     The splash holds the page until the heavy assets are actually in, so
     nothing pops in behind the visitor once they are reading. The bar tracks
     real bytes — a fake timer that always takes 3 seconds is worse than no bar,
     because it lies on both fast and slow connections.

     What is preloaded is deliberately just the above-the-fold set. The
     screenshots further down are ~2.5 MB and would hold a phone on the splash
     for many seconds to prefetch something the visitor may never scroll to.

     Two escape hatches, because a loader that can strand someone is a far worse
     bug than one that reveals slightly early: a hard timeout, and any click or
     key press. */

  const ASSETS = [
    { url: 'video/backdrop.mp4', weight: 3 },
    { url: 'audio/alien-theme.opus', weight: 6 },
    { url: 'alien.svg', weight: 1 },
  ];
  for (let i = 1; i <= 4; i++) ASSETS.push({ url: `audio/voice/0${i}.opus`, weight: 1 });

  const totalWeight = ASSETS.reduce((a, x) => a + x.weight, 0);
  let doneWeight = 0;
  let bootDone = false;

  function paintProgress() {
    const pct = Math.min(100, Math.round((doneWeight / totalWeight) * 100));
    if (bootFill) bootFill.style.width = `${pct}%`;
    if (bootPct) bootPct.textContent = `${pct}%`;
  }

  function preload() {
    return Promise.all(ASSETS.map((a) =>
      fetch(a.url, { cache: 'force-cache' })
        // Drain the body so the bytes are genuinely in the HTTP cache, not just
        // headers received.
        .then((r) => (r.ok ? r.blob() : null))
        .catch(() => null)
        .then(() => { doneWeight += a.weight; paintProgress(); })
    ));
  }

  function endBoot() {
    if (!boot || boot.classList.contains('done')) return;
    boot.classList.add('done');
    sessionStorage.setItem('alien-booted', '1');
    setTimeout(() => { boot.remove(); }, 800);
  }

  function runBoot() {
    let line = 0;
    const step = () => {
      if (line >= BOOT_LINES.length) { bootDone = true; return; }
      bootOut.textContent += BOOT_LINES[line] + '\n';
      if (BOOT_LINES[line].trim()) Sound.servo();
      line++;
      setTimeout(step, 95 + Math.random() * 90);
    };
    step();
  }

  if (!boot) {
    /* nothing to do */
  } else if (sessionStorage.getItem('alien-booted') || reduced) {
    boot.remove();
  } else {
    runBoot();
    paintProgress();

    // Reveal when the assets are in AND the boot text has finished, so the
    // curtain never lifts mid-sentence.
    const ready = preload();
    const waitForText = () => new Promise((res) => {
      const poll = () => (bootDone ? res() : setTimeout(poll, 120));
      poll();
    });
    Promise.all([ready, waitForText()]).then(() => setTimeout(endBoot, 450));

    boot.addEventListener('click', endBoot);
    window.addEventListener('keydown', endBoot, { once: true });
    // Hard ceiling. A throttled background tab, a stalled asset, anything —
    // nobody is left staring at a progress bar.
    setTimeout(endBoot, 9000);
  }

  /* ── Parallax ─────────────────────────────────────────────────────────────
     Three sources feed two numbers, --px and --py on :root, each in roughly
     -1..1. Every layer reads the same pair at a different depth, so the whole
     scene stays coherent no matter which source is driving.

     1. An idle wander that never stops, so the background always has some
        motion even on a phone sitting still on a desk.
     2. Pointer position on desktop.
     3. Device tilt on mobile — but ONLY where it costs nothing. iOS 13+
        gates DeviceOrientationEvent behind requestPermission(), which needs a
        gesture and throws a modal at someone who just wanted to read an
        install command. Not worth it for a parallax effect, so on iOS we
        simply do not ask and the idle wander carries it.

     Everything is written inside a rAF and never reads layout, so this cannot
     cause a reflow no matter how fast the events arrive. */

  if (!reduced) {
    const root = document.documentElement;
    let targetX = 0, targetY = 0;   // where the inputs want us
    let curX = 0, curY = 0;         // where we actually are, eased
    let pointerX = 0, pointerY = 0;
    let tiltX = 0, tiltY = 0;
    const started = performance.now();

    const clamp = (v) => Math.max(-1, Math.min(1, v));

    window.addEventListener('pointermove', (e) => {
      pointerX = clamp((e.clientX / window.innerWidth) * 2 - 1);
      pointerY = clamp((e.clientY / window.innerHeight) * 2 - 1);
    }, { passive: true });

    // Permission-free only. If requestPermission exists we are on iOS and we
    // deliberately do not call it.
    const DOE = window.DeviceOrientationEvent;
    if (DOE && typeof DOE.requestPermission !== 'function') {
      window.addEventListener('deviceorientation', (e) => {
        // gamma is left/right tilt, beta front/back. 30 degrees of tilt is a
        // comfortable full deflection to hold a phone at.
        if (e.gamma != null) tiltX = clamp(e.gamma / 30);
        if (e.beta != null) tiltY = clamp((e.beta - 45) / 30);
      }, { passive: true });
    }

    const tick = (now) => {
      const t = (now - started) / 1000;
      // Two incommensurable periods, so the wander never repeats visibly.
      const idleX = Math.sin(t / 11) * 0.35 + Math.sin(t / 4.3) * 0.12;
      const idleY = Math.cos(t / 13) * 0.3 + Math.cos(t / 5.7) * 0.1;

      targetX = clamp(idleX + pointerX * 0.55 + tiltX * 0.8);
      targetY = clamp(idleY + pointerY * 0.55 + tiltY * 0.8);

      // Ease toward the target rather than tracking it exactly: raw pointer
      // tracking feels twitchy and raw tilt data is noisy.
      curX += (targetX - curX) * 0.045;
      curY += (targetY - curY) * 0.045;

      root.style.setProperty('--px', curX.toFixed(4));
      root.style.setProperty('--py', curY.toFixed(4));
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }

  /* ── Background film layer ────────────────────────────────────────────────
     Opt-in and self-removing. No clip ships by default, so the element is
     dropped unless `video/backdrop.mp4` actually loads — a missing file must
     leave no empty node, no console noise and no layout effect.

     preload="none" until we know it is there, so a visitor never pays for a
     video that does not exist. Muted and playsinline because every browser
     blocks autoplay with sound, and a background layer must never be the
     reason a page starts making noise. */

  const film = document.getElementById('filmlayer');
  if (film) {
    const src = (film.dataset.src || '').trim();
    // Opt-in by design. An empty data-src means no clip is installed, and the
    // element is removed without issuing a request — shipping a permanent 404
    // to every visitor to keep a disabled feature wired up is not a trade
    // worth making.
    if (!src || reduced) {
      film.remove();
    } else {
      const drop = () => { try { film.remove(); } catch { /* already gone */ } };
      film.addEventListener('error', drop, { once: true });
      film.addEventListener('loadeddata', () => {
        film.classList.add('on');
        film.play().catch(drop);
      }, { once: true });

      film.src = src;
      film.preload = 'auto';
      film.load();
      // `error` does not fire for every failure mode; a source that never
      // resolves just leaves the element in NETWORK_NO_SOURCE forever.
      setTimeout(() => {
        if (!film.isConnected) return;
        if (film.readyState === 0) drop();
      }, 5000);
    }
  }

  /* ── The scenes ───────────────────────────────────────────────────────────
     Set dressing behind the whole page: a ship's console still reporting on a
     situation nobody is left to acknowledge. It is a rotation of themes rather
     than one fixed panel — each fades in, runs for a while, and fades away.

     All of it is decorative and aria-hidden, it never moves the layout, and it
     stops itself when the tab is hidden so a parked page costs nothing.

     The vocabulary is deliberately generic — "non-human bioform",
     "hunter-class", "acid breach" — rather than any franchise's proper nouns.
     The homage is in the tone; borrowing marks would put a real company's
     public site somewhere it does not need to be. */

  const scenesEl = document.getElementById('scenes');
  const alarmEl = document.getElementById('alarm');

  if (scenesEl && !reduced) {
    const pick = (a) => a[Math.floor(Math.random() * a.length)];
    const n = (a, b) => Math.floor(a + Math.random() * (b - a));
    const el = (tag, cls, text) => {
      const e = document.createElement(tag);
      if (cls) e.className = cls;
      if (text != null) e.textContent = text;
      return e;
    };

    const DECKS = ['A', 'B', 'C', 'D', 'E'];
    const place = () => `${pick(DECKS)}-${n(1, 10)}${pick(['', '', ' aft', ' fwd', ' port'])}`;

    const EVENTS = [
      ['', 'motion ~ · 3 contacts · non-human'],
      ['', 'internal sensor ~ recalibrated'],
      ['', 'atmos scrubber ~ cycling'],
      ['', 'cargo lift ~ manual override held'],
      ['', 'comms relay ~ carrier only · no voice'],
      ['warn', 'dock ~ seal integrity degraded'],
      ['warn', 'coolant loop ~ offline · hull temp rising'],
      ['warn', 'bioform ~ mass 118kg · gait irregular'],
      ['warn', 'hunter-class signature ~ · cloaked'],
      ['warn', 'thermal bloom ~ · plasma discharge'],
      ['warn', 'blast door ~ jammed · obstruction detected'],
      ['crit', 'DEPRESSURIZED ~ · atmosphere lost'],
      ['crit', 'ACID BREACH ~ · deck plating compromised'],
      ['crit', 'BIOSIGN LOST ~ · crew unaccounted for'],
      ['crit', 'HULL PENETRATION ~ · sealing failed'],
      ['crit', 'CONTAINMENT FAILURE ~ · specimen not in cell'],
      ['crit', 'DOCK ~ FORCED FROM OUTSIDE'],
    ];

    let clock = n(0, 60000);
    const stamp = () => {
      clock += n(7, 340);
      const h = String(Math.floor(clock / 3600) % 24).padStart(2, '0');
      const m = String(Math.floor(clock / 60) % 60).padStart(2, '0');
      const sec = String(clock % 60).padStart(2, '0');
      return `${h}:${m}:${sec}`;
    };

    function alarm() {
      alarmEl.classList.add('on');
      setTimeout(() => alarmEl.classList.remove('on'), 1400);
    }

    /* Each theme returns { node, timers }. The engine owns the fade and the
       teardown; a theme only has to build itself and say what it wants ticked. */

    const THEMES = {
      log() {
        const wrap = el('div', 's-log');
        const push = () => {
          const [sev, tpl] = pick(EVENTS);
          const row = el('div', sev);
          row.appendChild(el('span', 't', stamp()));
          row.appendChild(document.createTextNode(tpl.replace('~', place())));
          wrap.appendChild(row);
          while (wrap.children.length > 15) wrap.removeChild(wrap.firstChild);
          if (sev === 'crit') { alarm(); Sound.bg.squelch(); }
          else if (sev === 'warn') Sound.bg.servo();
        };
        for (let i = 0; i < 12; i++) push();   // arrive mid-incident
        return { node: wrap, tick: push, every: [1500, 3800] };
      },

      radar() {
        const wrap = el('div', 's-radar');
        wrap.appendChild(el('div', 'rings'));
        wrap.appendChild(el('div', 'sweep'));
        const blip = () => {
          const b = el('div', 'blip' + (Math.random() < 0.4 ? ' hostile' : ''));
          // Uniform over the disc, not over the radius — otherwise everything
          // clusters in the middle.
          const r = Math.sqrt(Math.random()) * 46;
          const a = Math.random() * Math.PI * 2;
          b.style.left = `${50 + r * Math.cos(a)}%`;
          b.style.top = `${50 + r * Math.sin(a)}%`;
          wrap.appendChild(b);
          setTimeout(() => b.remove(), 4200);
          Sound.bg.ping();
        };
        for (let i = 0; i < 4; i++) blip();
        return { node: wrap, tick: blip, every: [900, 2600] };
      },

      decks() {
        const wrap = el('div', 's-decks');
        const cells = [];
        for (let i = 0; i < 72; i++) { const c = el('i'); wrap.appendChild(c); cells.push(c); }
        const churn = () => {
          const c = pick(cells);
          const was = c.className;
          c.className = pick(['', '', '', 'breach', 'breach', 'vent', 'hot']);
          if (c.className === 'hot' && was !== 'hot') Sound.bg.servo();
        };
        // Already damaged on arrival. A ship that degrades from pristine while
        // you watch reads as a loading bar; one already in trouble reads as
        // history.
        for (let i = 0; i < 26; i++) churn();
        return { node: wrap, tick: churn, every: [500, 1600] };
      },

      vitals() {
        const wrap = el('div', 's-vitals');
        const M = [
          { k: 'o2 reserve', v: 41, lo: 16, hi: 46, u: '%' },
          { k: 'pressure', v: 62, lo: 28, hi: 78, u: 'kPa' },
          { k: 'reactor output', v: 88, lo: 68, hi: 96, u: '%' },
          { k: 'hull integrity', v: 54, lo: 34, hi: 70, u: '%' },
          { k: 'crew biosign', v: 2, lo: 2, hi: 3, u: '/19' },
        ];
        M.forEach((m) => {
          const row = el('div');
          row.appendChild(el('span', null, m.k));
          const bar = el('span', 'bar');
          m.fill = el('b');
          bar.appendChild(m.fill);
          row.appendChild(bar);
          m.val = el('span', 'val');
          row.appendChild(m.val);
          wrap.appendChild(row);
          m.bar = bar;
        });
        const drift = () => {
          M.forEach((m) => {
            // Biased downward: everything on this ship is getting worse.
            m.v = Math.max(m.lo, Math.min(m.hi, m.v + (Math.random() - 0.56) * 2.4));
            const pct = m.u === '/19' ? (m.v / 19) * 100 : m.v;
            m.fill.style.width = `${Math.max(2, pct)}%`;
            m.val.textContent = `${Math.round(m.v)}${m.u}`;
            m.bar.className = `bar ${pct < 30 ? 'crit' : pct < 55 ? 'low' : ''}`;
          });
        };
        drift();
        return { node: wrap, tick: drift, every: [1800, 3400] };
      },

      roster() {
        const NAMES = ['ARCARO J', 'BENNET T', 'CHIGARU N', 'DELACROIX M', 'ESPARZA R',
                       'FOWLER K', 'GRIEVES A', 'HALLORAN P', 'IKEDA S', 'JANSSEN W',
                       'KOVAC L', 'LUMET D', 'MBEKI O', 'NOVAK F', 'ORTIZ V',
                       'PARIS H', 'QUINTERO E', 'REYES B', 'SOKOLOV Y'];
        const wrap = el('div', 's-roster');
        const rows = NAMES.map((name) => {
          const d = el('div');
          d.appendChild(document.createTextNode(name + ' · '));
          const st = el('span', null, 'nominal');
          d.appendChild(st);
          wrap.appendChild(d);
          return { d, st };
        });
        // Seventeen of nineteen are already gone when the screen comes up.
        const order = rows.map((_, i) => i).sort(() => Math.random() - 0.5);
        let cursor = 0;
        const kill = () => {
          if (cursor >= order.length - 2) return;
          const r = rows[order[cursor++]];
          const lost = Math.random() < 0.72;
          r.d.className = lost ? 'lost' : 'unk';
          r.st.textContent = lost ? 'biosign lost' : 'no contact';
          if (lost) Sound.bg.squelch();
        };
        for (let i = 0; i < 13; i++) kill();
        return { node: wrap, tick: kill, every: [1400, 3000] };
      },

      wire() {
        // Pure vector: concentric polygons, an orbit ring, a hull outline and a
        // set of bearing ticks. Nothing filled — filled shapes at this size
        // read as a chart pasted over the page instead of a display behind it.
        const NS = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '-100 -100 200 200');

        const poly = (sides, r, cls, extra) => {
          const pts = [];
          for (let i = 0; i < sides; i++) {
            const a = (i / sides) * Math.PI * 2 - Math.PI / 2;
            pts.push(`${(Math.cos(a) * r).toFixed(2)},${(Math.sin(a) * r).toFixed(2)}`);
          }
          const e = document.createElementNS(NS, 'polygon');
          e.setAttribute('points', pts.join(' '));
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', '0.7');
          e.setAttribute('opacity', extra || '0.55');
          if (cls) e.setAttribute('class', cls);
          return e;
        };

        const circle = (r, dash, cls) => {
          const e = document.createElementNS(NS, 'circle');
          e.setAttribute('r', r);
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', '0.6');
          e.setAttribute('opacity', '0.4');
          if (dash) e.setAttribute('stroke-dasharray', dash);
          if (cls) e.setAttribute('class', cls);
          return e;
        };

        svg.appendChild(poly(6, 88, 'spin-slow', '0.5'));
        svg.appendChild(poly(3, 62, 'spin-rev', '0.45'));
        svg.appendChild(poly(6, 34, 'spin-slow pulse', '0.7'));
        svg.appendChild(circle(74, '2 5', 'spin-rev'));
        svg.appendChild(circle(46));

        // Bearing ticks around the outer ring.
        const ticks = document.createElementNS(NS, 'g');
        ticks.setAttribute('class', 'spin-slow');
        for (let i = 0; i < 36; i++) {
          const a = (i / 36) * Math.PI * 2;
          const l = document.createElementNS(NS, 'line');
          const r0 = i % 3 === 0 ? 90 : 94;
          l.setAttribute('x1', (Math.cos(a) * r0).toFixed(2));
          l.setAttribute('y1', (Math.sin(a) * r0).toFixed(2));
          l.setAttribute('x2', (Math.cos(a) * 98).toFixed(2));
          l.setAttribute('y2', (Math.sin(a) * 98).toFixed(2));
          l.setAttribute('stroke', 'currentColor');
          l.setAttribute('stroke-width', '0.6');
          l.setAttribute('opacity', '0.4');
          ticks.appendChild(l);
        }
        svg.appendChild(ticks);

        // A crosshair that jumps to a new bearing every tick — the only moving
        // part that is not on a fixed rotation, so the scene never looks looped.
        const cross = document.createElementNS(NS, 'g');
        cross.setAttribute('stroke', 'currentColor');
        cross.setAttribute('stroke-width', '0.7');
        cross.setAttribute('opacity', '0.85');
        const mk = (x1, y1, x2, y2) => {
          const l = document.createElementNS(NS, 'line');
          l.setAttribute('x1', x1); l.setAttribute('y1', y1);
          l.setAttribute('x2', x2); l.setAttribute('y2', y2);
          return l;
        };
        const box = document.createElementNS(NS, 'rect');
        box.setAttribute('width', '18'); box.setAttribute('height', '18');
        box.setAttribute('stroke', 'currentColor');
        box.setAttribute('stroke-width', '0.7');
        box.setAttribute('fill', 'none');
        cross.appendChild(box);
        cross.appendChild(mk(-26, 9, -4, 9));
        cross.appendChild(mk(22, 9, 44, 9));
        svg.appendChild(cross);

        const wrap = el('div', 's-wire');
        wrap.appendChild(svg);

        const move = () => {
          const r = Math.sqrt(Math.random()) * 62;
          const a = Math.random() * Math.PI * 2;
          const x = Math.cos(a) * r - 9;
          const y = Math.sin(a) * r - 9;
          box.setAttribute('x', x.toFixed(1));
          box.setAttribute('y', y.toFixed(1));
          cross.setAttribute('transform', `translate(${(x + 9).toFixed(1)} ${(y + 9).toFixed(1)})`);
          box.setAttribute('x', -9); box.setAttribute('y', -9);
          Sound.bg.ping();
        };
        move();
        return { node: wrap, tick: move, every: [1800, 4200] };
      },

      // Occult geometry: hexagram, vesica piscis, concentric rings and radial
      // spokes. Constructed from the same primitives as everything else, so it
      // is a drawing rather than a borrowed symbol.
      sigil() {
        const NS = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '-100 -100 200 200');
        const mk = (name, attrs, cls) => {
          const e = document.createElementNS(NS, name);
          for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', '0.7');
          if (cls) e.setAttribute('class', cls);
          return e;
        };
        const tri = (r, flip, cls) => {
          const pts = [];
          for (let i = 0; i < 3; i++) {
            const a = (i / 3) * Math.PI * 2 - Math.PI / 2 + (flip ? Math.PI : 0);
            pts.push(`${(Math.cos(a) * r).toFixed(2)},${(Math.sin(a) * r).toFixed(2)}`);
          }
          return mk('polygon', { points: pts.join(' '), opacity: 0.6 }, cls);
        };

        svg.appendChild(mk('circle', { r: 92, opacity: 0.35 }, 'spin-c'));
        svg.appendChild(mk('circle', { r: 68, opacity: 0.3, 'stroke-dasharray': '1 4' }, 'spin-b'));
        svg.appendChild(tri(74, false, 'spin-a'));
        svg.appendChild(tri(74, true, 'spin-b'));

        // Vesica piscis: two circles each passing through the other's centre.
        const g = document.createElementNS(NS, 'g');
        g.setAttribute('class', 'breathe');
        g.appendChild(mk('circle', { r: 34, cx: -17, opacity: 0.5 }));
        g.appendChild(mk('circle', { r: 34, cx: 17, opacity: 0.5 }));
        svg.appendChild(g);

        const spokes = document.createElementNS(NS, 'g');
        spokes.setAttribute('class', 'spin-a');
        for (let i = 0; i < 12; i++) {
          const a = (i / 12) * Math.PI * 2;
          spokes.appendChild(mk('line', {
            x1: (Math.cos(a) * 78).toFixed(2), y1: (Math.sin(a) * 78).toFixed(2),
            x2: (Math.cos(a) * 92).toFixed(2), y2: (Math.sin(a) * 92).toFixed(2),
            opacity: 0.4,
          }));
        }
        svg.appendChild(spokes);

        const wrap = el('div', 's-sigil');
        wrap.appendChild(svg);
        const pulse = () => { Sound.bg.servo(); };
        return { node: wrap, tick: pulse, every: [3000, 7000] };
      },

      // A polygon that will not settle: it interpolates between vertex counts,
      // so the shape is always partway between two solids it never becomes.
      morph() {
        const NS = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '-100 -100 200 200');
        const form = document.createElementNS(NS, 'path');
        form.setAttribute('class', 'form');
        form.setAttribute('stroke', 'currentColor');
        form.setAttribute('stroke-width', '0.8');
        form.setAttribute('opacity', '0.75');
        svg.appendChild(form);

        const inner = document.createElementNS(NS, 'path');
        inner.setAttribute('stroke', 'currentColor');
        inner.setAttribute('stroke-width', '0.6');
        inner.setAttribute('opacity', '0.4');
        svg.appendChild(inner);

        // Sampled at a fixed high resolution and re-shaped by a radius
        // function, so any two "shapes" have matching point counts and the
        // interpolation between them is continuous.
        const N = 160;
        const radius = (theta, sides, r) => {
          const seg = Math.PI * 2 / sides;
          const a = ((theta % seg) + seg) % seg - seg / 2;
          return r * Math.cos(seg / 2) / Math.cos(a);
        };
        const build = (sides, r) => {
          let d = '';
          for (let i = 0; i <= N; i++) {
            const th = (i / N) * Math.PI * 2;
            const rr = radius(th, sides, r);
            d += (i ? 'L' : 'M') + (Math.cos(th) * rr).toFixed(2) + ',' + (Math.sin(th) * rr).toFixed(2);
          }
          return d + 'Z';
        };

        let from = 3, to = 7, phase = 0;
        const step = () => {
          phase += 0.06;
          if (phase >= 1) { phase = 0; from = to; to = 3 + Math.floor(Math.random() * 7); }
          const sides = from + (to - from) * phase;
          form.setAttribute('d', build(sides, 78));
          inner.setAttribute('d', build(sides, 44));
        };
        step();

        const wrap = el('div', 's-morph');
        wrap.appendChild(svg);
        // Fast tick: this one is a continuous animation driven from JS rather
        // than a state change, so it needs frames, not events.
        return { node: wrap, tick: step, every: [60, 90] };
      },

      // Star chart: an ephemeris nobody is left to read.
      chart() {
        const NS = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '-100 -100 200 200');
        const stroke = (e, w, o) => {
          e.setAttribute('stroke', 'currentColor');
          e.setAttribute('stroke-width', w);
          e.setAttribute('opacity', o);
          e.setAttribute('fill', 'none');
          return e;
        };
        // Orbits: ellipses at assorted inclinations.
        const orbits = document.createElementNS(NS, 'g');
        orbits.setAttribute('class', 'spin-c');
        for (let i = 0; i < 5; i++) {
          const e = document.createElementNS(NS, 'ellipse');
          e.setAttribute('rx', 26 + i * 15);
          e.setAttribute('ry', (26 + i * 15) * (0.3 + i * 0.12));
          e.setAttribute('transform', `rotate(${i * 34})`);
          orbits.appendChild(stroke(e, '0.55', 0.35));
        }
        svg.appendChild(orbits);

        // Stars, and the lines someone drew between them.
        const pts = [];
        for (let i = 0; i < 14; i++) {
          const r = Math.sqrt(Math.random()) * 88;
          const a = Math.random() * Math.PI * 2;
          pts.push([Math.cos(a) * r, Math.sin(a) * r]);
        }
        const links = document.createElementNS(NS, 'g');
        for (let i = 0; i < pts.length - 1; i++) {
          if (Math.random() < 0.45) continue;
          const l = document.createElementNS(NS, 'line');
          l.setAttribute('x1', pts[i][0].toFixed(1));
          l.setAttribute('y1', pts[i][1].toFixed(1));
          l.setAttribute('x2', pts[i + 1][0].toFixed(1));
          l.setAttribute('y2', pts[i + 1][1].toFixed(1));
          links.appendChild(stroke(l, '0.5', 0.3));
        }
        svg.appendChild(links);

        const stars = document.createElementNS(NS, 'g');
        pts.forEach(([x, y]) => {
          const c = document.createElementNS(NS, 'circle');
          c.setAttribute('cx', x.toFixed(1));
          c.setAttribute('cy', y.toFixed(1));
          c.setAttribute('r', (0.8 + Math.random() * 1.6).toFixed(2));
          stars.appendChild(stroke(c, '0.6', 0.7));
        });
        svg.appendChild(stars);

        const wrap = el('div', 's-chart');
        wrap.appendChild(svg);
        const twinkle = () => {
          const c = stars.children[Math.floor(Math.random() * stars.children.length)];
          if (!c) return;
          c.setAttribute('opacity', (0.25 + Math.random() * 0.7).toFixed(2));
        };
        return { node: wrap, tick: twinkle, every: [280, 900] };
      },

      // A glyph grid that keeps rewriting itself. The characters are drawn from
      // Unicode's geometric and technical blocks — shapes, not any real script.
      glyphs() {
        const POOL = [...'◇◈◉◊○◌◍◎●◐◑◒◓◔◕◖◗◘◙◚◛◜◝◞◟◠◡◢◣◤◥▲△▴▵▶▷▸▹►▻▼▽▾▿◀◁◂◃',
                      ...'⌁⌂⌆⌇⌘⌑⌒⌓⌔⌕⌖⌗⌘⌙⌬⌭⌮⌯⌰⌱⌲⌳⌴⌵⌶⌷⌸⌹⌺⌻⌼⌽⌾⌿⍀⍁⍂⍃⍄⍅⍆⍇⍈⍉⍊⍋⍌'];
        const wrap = el('div', 's-glyphs');
        const cells = [];
        for (let i = 0; i < 60; i++) {
          const c = el('span', null, POOL[Math.floor(Math.random() * POOL.length)]);
          c.style.setProperty('--o', (0.2 + Math.random() * 0.6).toFixed(2));
          wrap.appendChild(c);
          cells.push(c);
        }
        const shuffle = () => {
          for (let i = 0; i < 3; i++) {
            const c = cells[Math.floor(Math.random() * cells.length)];
            c.textContent = POOL[Math.floor(Math.random() * POOL.length)];
            c.style.setProperty('--o', (0.15 + Math.random() * 0.65).toFixed(2));
          }
        };
        return { node: wrap, tick: shuffle, every: [220, 700] };
      },

      docks() {
        const BAYS = ['dock 01', 'dock 02', 'dock 03', 'airlock a', 'airlock b',
                      'cargo ramp', 'escape pod 1', 'escape pod 2'];
        const STATES = [
          ['ok', 'sealed'], ['ok', 'sealed'], ['warn', 'cycling'],
          ['warn', 'seal degraded'], ['crit', 'FORCED — OPEN TO VACUUM'],
          ['crit', 'DEPRESSURIZED'], ['warn', 'manual override'],
          ['crit', 'LAUNCHED — UNAUTHORISED'],
        ];
        const wrap = el('div', 's-docks');
        const rows = BAYS.map((bay) => {
          const d = el('div');
          d.appendChild(el('span', null, bay));
          const st = el('span');
          const code = el('span', null, `#${n(1000, 9999)}`);
          d.appendChild(st); d.appendChild(code);
          wrap.appendChild(d);
          return st;
        });
        const set = () => {
          const st = pick(rows);
          const [cls, text] = pick(STATES);
          st.className = cls;
          st.textContent = text;
          if (cls === 'crit') { alarm(); Sound.bg.squelch(); }
        };
        for (let i = 0; i < 8; i++) set();
        return { node: wrap, tick: set, every: [1600, 3600] };
      },
    };

    const TITLES = {
      log: 'incident log // unacknowledged',
      radar: 'motion tracker // deck sweep',
      decks: 'compartment integrity',
      vitals: 'life support',
      roster: 'crew manifest',
      docks: 'dock and airlock status',
      wire: 'navigation lattice // dead reckoning',
      sigil: 'containment sigil // origin unknown',
      morph: 'geometry // unstable solid',
      chart: 'star chart // ephemeris',
      glyphs: 'inscription // untranslated',
    };

    const NAMES = Object.keys(THEMES);
    let current = null;
    let running = false;
    let cycleTimer = null;

    function teardown(scene) {
      if (!scene) return;
      scene.timers.forEach(clearTimeout);
      scene.node.classList.remove('on');
      setTimeout(() => scene.node.remove(), 2600);
    }

    function show(name) {
      const built = THEMES[name]();
      const node = el('div', 'scene');
      node.appendChild(el('div', 'scene-title', TITLES[name]));
      node.appendChild(built.node);
      scenesEl.appendChild(node);

      const scene = { node, timers: [] };
      // Next frame, so the browser has a chance to paint opacity:0 first and
      // actually run the transition rather than snapping straight to visible.
      requestAnimationFrame(() => requestAnimationFrame(() => node.classList.add('on')));

      const loop = () => {
        built.tick();
        scene.timers.push(setTimeout(loop, n(built.every[0], built.every[1])));
      };
      scene.timers.push(setTimeout(loop, n(built.every[0], built.every[1])));
      return scene;
    }

    let lastName = null;
    function cycle() {
      if (!running) return;
      let name = pick(NAMES);
      // Never the same theme twice in a row; with six of them, resampling is
      // simpler than shuffling and the bias is invisible.
      while (name === lastName && NAMES.length > 1) name = pick(NAMES);
      lastName = name;

      const previous = current;
      current = show(name);
      teardown(previous);

      cycleTimer = setTimeout(cycle, n(11000, 17000));
    }

    function start() { if (running) return; running = true; cycle(); }
    function stop() {
      running = false;
      clearTimeout(cycleTimer);
      teardown(current);
      current = null;
    }

    // A hidden tab throttles timers anyway; stopping outright is honest about
    // it and keeps a parked tab off the CPU entirely.
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stop(); else start();
    });

    start();
  }

  /* ── Ambient layer ────────────────────────────────────────────────────────
     Everything that happens away from the centred scene: small readouts at
     varying depths, the display faults, the announcer, and the thing moving
     through the shadows. Independent of the scene rotation on purpose — if it
     all pulsed on one clock the page would breathe in unison and read as a
     single looping animation. */

  const motesEl = document.getElementById('motes');
  const passerEl = document.getElementById('passer');
  const dropoutEl = document.getElementById('dropout');

  if (motesEl && !reduced) {
    const pick = (a) => a[Math.floor(Math.random() * a.length)];
    const n = (a, b) => a + Math.random() * (b - a);
    const root = document.documentElement;

    const MOTES = [
      ['', 'PWR 41.2V\nDRAW 18.6A'],
      ['', 'SCRUB CYC 04\nCO2 0.41%'],
      ['', 'GRAV 0.98G\nTRIM NOMINAL'],
      ['', 'RELAY 7\nCARRIER ONLY'],
      ['', 'NAV LOCK\nDEAD RECKONING'],
      ['', 'CAM 12\nSIGNAL LOST'],
      ['warn', 'TEMP 61C\nRISING'],
      ['warn', 'SEAL 03\nDEGRADED'],
      ['warn', 'MOTION\n3 CONTACTS'],
      ['warn', 'MASS 118KG\nUNIDENTIFIED'],
      ['crit', 'O2 34%\nFALLING'],
      ['crit', 'BREACH\nDECK 4'],
      ['crit', 'VACUUM\nBAY 02'],
      ['crit', 'NO BIOSIGN\nSECTOR 9'],
    ];

    function mote() {
      const [sev, text] = pick(MOTES);
      const d = document.createElement('div');
      d.className = `mote ${sev}`;
      d.textContent = text;

      // Depth: further away means smaller, blurrier and fainter. Tying all
      // three to one value is what makes it read as distance rather than as
      // three unrelated random styles.
      const depth = Math.random();
      const scale = 0.7 + depth * 0.9;
      // Fainter and blurrier than before across the whole range: even the
      // nearest mote should be something you notice, not something you read.
      d.style.setProperty('--o', (0.08 + depth * 0.2).toFixed(2));
      d.style.setProperty('--drift', `${(13 + Math.random() * 11).toFixed(1)}s`);
      d.style.setProperty('--flick', `${(3.4 + Math.random() * 5).toFixed(1)}s`);
      d.style.transform = `scale(${scale.toFixed(2)})`;
      d.style.filter = `blur(${(4.4 - depth * 2.6).toFixed(2)}px)`;

      // Keep the middle of the screen clear: the product lives there.
      let x, y;
      do {
        x = n(2, 92);
        y = n(6, 88);
      } while (x > 24 && x < 76 && y > 26 && y < 74);
      d.style.left = `${x.toFixed(1)}%`;
      d.style.top = `${y.toFixed(1)}%`;

      motesEl.appendChild(d);
      requestAnimationFrame(() => requestAnimationFrame(() => d.classList.add('on')));
      setTimeout(() => d.classList.remove('on'), n(5000, 11000));
      setTimeout(() => d.remove(), 14000);
    }

    /* ── Display faults ──────────────────────────────────────────────────── */

    function fault() {
      const kind = Math.random();
      if (kind < 0.45) {
        root.classList.add('glitch');
        Sound.bg.statics(0.12);
        setTimeout(() => root.classList.remove('glitch'), 260);
      } else if (kind < 0.75) {
        root.classList.add('rgbsplit');
        setTimeout(() => root.classList.remove('rgbsplit'), 300);
      } else if (kind < 0.92) {
        dropoutEl.classList.add('on');
        Sound.bg.statics(0.3);
        setTimeout(() => dropoutEl.classList.remove('on'), 360);
      } else {
        root.classList.add('roll');
        Sound.bg.statics(0.2);
        setTimeout(() => root.classList.remove('roll'), 760);
      }
    }

    /* ── Announcer ───────────────────────────────────────────────────────── */

    const LINES = [
      'Roger.', 'Confirm.', 'Negative.', 'Say again.',
      'Warning.', 'Intrusion detected.', 'Evacuate.',
      'Motion detected. Sector seven.', 'Contact lost.',
      'Hull breach. Deck four.', 'Atmosphere critical.',
      'Containment failure.', 'Sealing bulkhead.',
      'All personnel evacuate immediately.',
      'Do not open that door.', 'Life support offline.',
      'Emergency power engaged.', 'It is inside.',
    ];

    function announce() { Sound.bg.say(pick(LINES)); }

    /* ── The passer-by ───────────────────────────────────────────────────── */

    let passing = false;
    function pass() {
      if (passing) return;
      passing = true;
      const loom = Math.random() < 0.4;
      passerEl.className = 'passer';
      // Force a reflow, or the class swap is coalesced and the animation never
      // restarts a second time.
      void passerEl.offsetWidth;
      if (!loom && Math.random() < 0.5) passerEl.classList.add('flip');
      passerEl.classList.add(loom ? 'loom' : 'cross');

      // A wet sound at the closest point, not at the start: the shape and the
      // sound arriving together is what makes it read as one event.
      setTimeout(() => Sound.bg.squelch(), loom ? 6800 : 8200);
      setTimeout(() => {
        passerEl.className = 'passer';
        passing = false;
      }, loom ? 13200 : 17200);
    }

    /* ── Clocks ──────────────────────────────────────────────────────────── */

    let timers = [];
    const loop = (fn, lo, hi) => {
      const tick = () => { fn(); timers.push(setTimeout(tick, n(lo, hi))); };
      timers.push(setTimeout(tick, n(lo, hi)));
    };

    function startAmbient() {
      if (timers.length) return;
      loop(mote, 900, 2600);
      loop(fault, 6000, 17000);
      loop(announce, 14000, 32000);
      loop(pass, 26000, 62000);
    }
    function stopAmbient() { timers.forEach(clearTimeout); timers = []; }

    document.addEventListener('visibilitychange', () => {
      if (document.hidden) stopAmbient(); else startAmbient();
    });

    for (let i = 0; i < 5; i++) mote();
    startAmbient();
    setTimeout(pass, 12000);
  }

  /* ── Copy buttons ─────────────────────────────────────────────────────── */

  document.querySelectorAll('[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const text = btn.getAttribute('data-copy');
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        // Clipboard API needs a secure context and a permission that can be
        // refused. Fall back rather than lying about having copied.
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.setAttribute('readonly', '');
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        // Belt and braces against the page-wide `user-select: none`: the CSS
        // exempts textarea, and this survives even if that rule is ever
        // tightened. Set through CSSOM, which CSP does not police — only
        // parsed style="" attributes are subject to style-src.
        ta.style.userSelect = 'text';
        ta.style.webkitUserSelect = 'text';
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand('copy'); } catch { /* nothing left to try */ }
        ta.remove();
      }
      const was = btn.textContent;
      btn.textContent = 'copied';
      btn.classList.add('ok');
      Sound.confirm();
      setTimeout(() => { btn.textContent = was; btn.classList.remove('ok'); }, 1600);
    });
  });

  /* ── Install tabs ─────────────────────────────────────────────────────── */

  const tabs = Array.from(document.querySelectorAll('.tab'));
  tabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      tabs.forEach((t) => {
        const sel = t === tab;
        t.setAttribute('aria-selected', String(sel));
        document.getElementById(t.getAttribute('aria-controls')).classList.toggle('on', sel);
      });
      Sound.servo();
    });
  });

  /* ── Thermal pointer ──────────────────────────────────────────────────── */

  document.querySelectorAll('.card').forEach((card) => {
    card.addEventListener('pointermove', (e) => {
      const r = card.getBoundingClientRect();
      card.style.setProperty('--mx', `${e.clientX - r.left}px`);
      card.style.setProperty('--my', `${e.clientY - r.top}px`);
    });
    let armed = true;
    card.addEventListener('pointerenter', () => {
      if (!armed) return;
      armed = false;
      Sound.squelch();
      setTimeout(() => { armed = true; }, 500);
    });
  });

  /* ── Reveal ───────────────────────────────────────────────────────────── */

  // The hidden state is armed here and nowhere else. Everything above has
  // already run, so if any of it had thrown, this line would never execute and
  // the page would simply render without the animation — visible, which is the
  // only acceptable failure mode.
  if (!reduced && 'IntersectionObserver' in window) {
    document.documentElement.classList.add('reveal-ready');

    const io = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        entry.target.classList.add('seen');
        io.unobserve(entry.target);
        Sound.bg.ping();
      });
    }, { threshold: 0.15 });

    const targets = document.querySelectorAll('.reveal');
    targets.forEach((el) => io.observe(el));

    // Belt and braces for anything the observer never gets to fire for: a
    // deep link that lands mid-page, a throttled background tab, a headless
    // renderer. After two seconds, show everything regardless.
    setTimeout(() => targets.forEach((el) => el.classList.add('seen')), 2000);
  }

  /* ── Hold the page at 1x ────────────────────────────────────────────────
     The viewport meta asks for this; these handlers are what enforce it in the
     two places the meta tag has no say.

     Pinch on a trackpad does not arrive as a gesture — it arrives as a `wheel`
     event with ctrlKey set, which is indistinguishable from a real ctrl-scroll
     and is how every browser implements trackpad zoom. It has to be passive:
     false or preventDefault() is ignored.

     `gesturestart` is the Safari-only pinch on touch and trackpad. Safari is
     the browser that ignores user-scalable outright, so this is the only thing
     that stops it there.

     What is NOT preventable, in any browser, is chrome-level zoom: ⌘/Ctrl with
     +, - or 0, and the menu item. The keydown never reaches the page. That is
     browser UI, deliberately outside a page's reach, and no amount of
     JavaScript changes it — so it is left alone rather than half-handled. */
  const blockZoom = (e) => { if (e.ctrlKey || e.metaKey) e.preventDefault(); };
  window.addEventListener('wheel', blockZoom, { passive: false });
  ['gesturestart', 'gesturechange', 'gestureend'].forEach((ev) =>
    document.addEventListener(ev, (e) => e.preventDefault(), { passive: false }));

  /* Selection is off in CSS, but a drag still fires selectstart in some engines
     and a long-press still opens a context menu over text on Android. Cancel
     both — except over the controls that need them. */
  document.addEventListener('selectstart', (e) => {
    if (e.target.closest && e.target.closest('textarea, input, [contenteditable]')) return;
    e.preventDefault();
  });
})();
