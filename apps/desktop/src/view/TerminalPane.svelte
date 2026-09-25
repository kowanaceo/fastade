<script lang="ts">
  import { onMount } from 'svelte';
  import { FitAddon } from '@xterm/addon-fit';
  import { Terminal } from '@xterm/xterm';
  import '@xterm/xterm/css/xterm.css';
  import { installWebKitHangulImeAdapter } from './webkit-hangul-ime';

  export let sessionId: string;
  export let registerOutput: (sessionId: string, sink: (data: string, replay?: boolean) => void) => () => void;
  export let onInput: (sessionId: string, data: string) => void;
  export let onResize: (sessionId: string, cols: number, rows: number) => void;
  export let onInterrupt: (sessionId: string) => void;
  export let onFocus: (sessionId: string) => void;
  export let onCurrentDirectory: (sessionId: string, path: string) => void;
  export let fontSize = 11;
  export let layoutRevision = 0;

  let container: HTMLDivElement;
  let terminalRef: Terminal | undefined;
  let fitRef: FitAddon | undefined;
  let scheduleFitRef: (() => void) | undefined;

  $: if (terminalRef && terminalRef.options.fontSize !== fontSize) {
    terminalRef.options.fontSize = fontSize;
    scheduleFitRef?.();
  }

  // Native window resize/scale events are more reliable than DOM resize events
  // while WKWebView is entering fullscreen or moving between displays.
  $: if (terminalRef && layoutRevision > 0) scheduleFitRef?.();

  onMount(() => {
    const terminal = new Terminal({
      allowProposedApi: false,
      cursorBlink: true,
      convertEol: false,
      fontFamily: 'SFMono-Regular, Menlo, Monaco, Consolas, "Apple SD Gothic Neo", "Noto Sans Mono CJK KR", monospace',
      fontSize,
      lineHeight: 1.15,
      overviewRuler: { width: 5 },
      scrollback: 5000,
      theme: {
        background: '#0b100d',
        foreground: '#d6e0d9',
        cursor: '#8eefaa',
        scrollbarSliderBackground: '#34413999',
        scrollbarSliderHoverBackground: '#526158cc',
        scrollbarSliderActiveBackground: '#627268e6',
        selectionBackground: '#385744',
      },
    });
    terminalRef = terminal;
    const fit = new FitAddon();
    fitRef = fit;
    terminal.loadAddon(fit);
    terminal.open(container);
    const removeHangulImeAdapter = installWebKitHangulImeAdapter(
      terminal,
      (data) => onInput(sessionId, data),
    );
    let replayWrites = 0;
    const unregister = registerOutput(sessionId, (data, replay = false) => {
      if (!replay) {
        terminal.write(data);
        return;
      }
      replayWrites += 1;
      terminal.write(data, () => { replayWrites -= 1; });
    });
    const input = terminal.onData((data) => {
      // Focus reports (DECSET 1004) fire whenever the user switches to another
      // session pane. Claude Code pauses its spinner while unfocused, and that
      // output silence would read as idle, so a still-working agent turned
      // green as soon as it was not the pane being watched. Every pane stays
      // "focused" from the agent's point of view instead.
      if (data === '\x1b[I' || data === '\x1b[O') return;
      // xterm emits replies to OSC color, cursor-position, and device-attribute
      // queries through onData. Queries in a historical transcript must not
      // inject those replies into the live shell when the pane is mounted.
      if (replayWrites === 0) onInput(sessionId, data);
    });
    const handlePaste = (event: ClipboardEvent): void => {
      if (event.target !== terminal.textarea) return;
      const text = event.clipboardData?.getData('text/plain');
      if (!text) return;
      // WKWebView does not reliably complete xterm's hidden-textarea paste
      // path. Feed the user-initiated clipboard payload through xterm so its
      // bracketed-paste handling remains intact.
      event.preventDefault();
      event.stopImmediatePropagation();
      terminal.paste(text);
    };
    terminal.element?.addEventListener('paste', handlePaste, { capture: true });
    // Private OSC number (must match FASTADE_CWD_OSC in app-view-model.svelte.ts),
    // not the standard OSC 7 "report cwd" code — an AI CLI running inside this
    // shell (Claude Code, Codex, Gemini) may itself emit real OSC 7 for its own
    // terminal integration, which would otherwise be misread as "the outer
    // shell's prompt came back" and falsely mark a still-running agent as exited.
    const cwd = terminal.parser.registerOscHandler(55123, (data) => {
      if (!data) return false;
      onCurrentDirectory(sessionId, data);
      return true;
    });
    let lastSyncedSize = '';
    const syncPtySize = (cols: number, rows: number): void => {
      // A session can be visible in both the main window and a pop-out. Only
      // the focused window owns the live PTY size; otherwise two observers
      // continually overwrite each other with different rows and columns.
      if (!document.hasFocus() || cols < 1 || rows < 1) return;
      const size = `${cols}x${rows}`;
      if (size === lastSyncedSize) return;
      lastSyncedSize = size;
      onResize(sessionId, cols, rows);
    };
    const resized = terminal.onResize(({ cols, rows }) => {
      syncPtySize(cols, rows);
    });
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.key !== 'Escape' || event.type !== 'keydown') return true;
      event.preventDefault();
      event.stopPropagation();
      onInterrupt(sessionId);
      return false;
    });
    let fitFrame = 0;
    let fitTimers: Array<ReturnType<typeof setTimeout>> = [];
    const fitAndRefresh = (): void => {
      try {
        fit.fit();
        // xterm may have accepted its new geometry while the native window
        // briefly had no focus during a fullscreen/display transition. In
        // that case onResize deliberately skipped the PTY update, and a later
        // fit() is a no-op. Explicitly synchronize the current geometry once
        // this window owns the session again.
        syncPtySize(terminal.cols, terminal.rows);
        // fit() is a no-op when the row/column count stays the same. A forced
        // redraw is still needed after the WebView backing scale changes.
        if (terminal.rows > 0) terminal.refresh(0, terminal.rows - 1);
      } catch { /* hidden terminal; retry on next resize */ }
    };
    const scheduleFit = (): void => {
      cancelAnimationFrame(fitFrame);
      fitTimers.forEach((timer) => clearTimeout(timer));
      fitTimers = [];
      fitFrame = requestAnimationFrame(() => {
        // A second frame observes the final content size after macOS finishes
        // a fullscreen animation or moves the window between display scales.
        fitFrame = requestAnimationFrame(() => {
          fitAndRefresh();
        });
      });
      // Native fullscreen transitions can finish well after ResizeObserver's
      // last callback. Re-fit through the end of that transition.
      fitTimers = [120, 350, 700].map((delay) => setTimeout(fitAndRefresh, delay));
    };
    scheduleFitRef = scheduleFit;
    const observer = new ResizeObserver(scheduleFit);
    const handleWindowFocus = (): void => {
      // Another focused window may have changed this shared PTY while this
      // pane was inactive, so reclaim ownership even when its size is equal
      // to the last size reported from this pane.
      lastSyncedSize = '';
      scheduleFit();
    };
    let resolutionQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    const handleResolutionChange = (): void => {
      resolutionQuery.removeEventListener('change', handleResolutionChange);
      resolutionQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      resolutionQuery.addEventListener('change', handleResolutionChange);
      scheduleFit();
    };
    observer.observe(container);
    window.addEventListener('resize', scheduleFit);
    window.addEventListener('focus', handleWindowFocus);
    window.visualViewport?.addEventListener('resize', scheduleFit);
    resolutionQuery.addEventListener('change', handleResolutionChange);
    scheduleFit();
    requestAnimationFrame(() => terminal.focus());
    return () => {
      cancelAnimationFrame(fitFrame);
      fitTimers.forEach((timer) => clearTimeout(timer));
      scheduleFitRef = undefined;
      observer.disconnect();
      window.removeEventListener('resize', scheduleFit);
      window.removeEventListener('focus', handleWindowFocus);
      window.visualViewport?.removeEventListener('resize', scheduleFit);
      resolutionQuery.removeEventListener('change', handleResolutionChange);
      removeHangulImeAdapter();
      unregister();
      input.dispose();
      terminal.element?.removeEventListener('paste', handlePaste, { capture: true });
      cwd.dispose();
      resized.dispose();
      terminal.dispose();
      terminalRef = undefined;
      fitRef = undefined;
    };
  });
</script>

<div
  class="terminal-host"
  role="application"
  aria-label="Terminal"
  bind:this={container}
  onfocusin={() => onFocus(sessionId)}
  oncontextmenu={(event) => event.stopPropagation()}
></div>

<style>
  /* Keep the final TUI status row clear of WebKit's clipped bottom edge.
     FitAddon only subtracts padding from `.xterm` itself, not its parent.
     Keeping the padding on the host made the PTY one row and a few columns
     larger than the visible screen, which displaced Codex's composer cursor
     and caused bottom-anchored choice prompts to overlap their content. */
  .terminal-host { width: 100%; height: 100%; min-width: 0; min-height: 0; overflow: hidden; background: #0b100d; }
  :global(.xterm) { width: 100%; height: 100%; padding: 0 0 11px; }
  /* xterm.css paints the viewport #000, which shows through the bottom padding. */
  :global(.xterm .xterm-viewport) { background-color: #0b100d; scrollbar-color: #34413999 transparent; scrollbar-width: thin; }
</style>
