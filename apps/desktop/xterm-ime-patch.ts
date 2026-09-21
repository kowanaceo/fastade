import type { Plugin } from 'vite';

const XTERM_MODULE_SUFFIX = '/@xterm/xterm/lib/xterm.mjs';

/**
 * Backport xterm.js PR #6090 to the currently published 6.0.0 bundle.
 *
 * xterm.js uses one boolean for every deferred IME composition send. When
 * Korean composition events arrive back-to-back, finalizing the newest input
 * clears that boolean and silently cancels older committed syllables. Keep a
 * FIFO of pending sends and a watermark instead, matching the upstream fix.
 *
 * Remove this plugin after a released @xterm/xterm version contains:
 * https://github.com/xtermjs/xterm.js/pull/6090
 */
export function xtermImePatch(): Plugin {
  return {
    name: 'xterm-korean-ime-backport',
    enforce: 'pre',
    transform(code, id) {
      if (!id.replaceAll('\\', '/').endsWith(XTERM_MODULE_SUFFIX)) return null;

      let patched = replaceExactlyOnce(
        code,
        'this._isComposing=!1,this._isSendingComposition=!1,this._compositionPosition={start:0,end:0},this._dataAlreadySent=""',
        'this._isComposing=!1,this._pendingSends=[],this._sentUpTo=0,this._compositionPosition={start:0,end:0},this._dataAlreadySent=""',
        'constructor state',
      );
      patched = replaceExactlyOnce(
        patched,
        'this._compositionPosition.start=this._textarea.value.length,this._compositionView.textContent=""',
        'this._compositionPosition.start=this._textarea.value.length,this._sentUpTo=Math.min(this._sentUpTo,this._compositionPosition.start),this._compositionView.textContent=""',
        'composition watermark reset',
      );
      patched = replaceExactlyOnce(
        patched,
        'if(this._isComposing||this._isSendingComposition)',
        'if(this._isComposing||this._pendingSends.length>0)',
        'keydown composition guard',
      );

      const oldFinalize = '_finalizeComposition(t){if(this._compositionView.classList.remove("active"),this._isComposing=!1,t){let e={start:this._compositionPosition.start,end:this._compositionPosition.end};this._isSendingComposition=!0,setTimeout(()=>{if(this._isSendingComposition){this._isSendingComposition=!1;let i;e.start+=this._dataAlreadySent.length,this._isComposing?i=this._textarea.value.substring(e.start,this._compositionPosition.start):i=this._textarea.value.substring(e.start),i.length>0&&this._coreService.triggerDataEvent(i,!0)}},0)}else{this._isSendingComposition=!1;let e=this._textarea.value.substring(this._compositionPosition.start,this._compositionPosition.end);this._coreService.triggerDataEvent(e,!0)}}_handleAnyTextareaChanges';
      const newFinalize = '_finalizeComposition(t){if(this._compositionView.classList.remove("active"),this._isComposing=!1,t){let e={start:this._compositionPosition.start,end:this._compositionPosition.end},i=()=>{e.start+=this._dataAlreadySent.length;let t=this._textarea.value,r=this._compositionPosition.start>e.start?this._compositionPosition.start:t.length,n=this._sliceUnsent(e.start,r);n.length>0&&this._coreService.triggerDataEvent(n,!0)};this._pendingSends.push(i),setTimeout(()=>{let t=this._pendingSends.indexOf(i);t!==-1&&(this._pendingSends.splice(t,1),i())},0)}else{for(let t of this._pendingSends.splice(0,this._pendingSends.length))t();let e=Math.max(this._compositionPosition.end,this._textarea.selectionEnd??this._compositionPosition.end),i=this._sliceUnsent(this._compositionPosition.start,e);i.length>0&&this._coreService.triggerDataEvent(i,!0)}}_sliceUnsent(t,e){let i=Math.max(t,this._sentUpTo),r=Math.max(i,e);return this._sentUpTo=Math.max(this._sentUpTo,r),this._textarea.value.substring(i,r)}_handleAnyTextareaChanges';
      patched = replaceExactlyOnce(patched, oldFinalize, newFinalize, 'composition send queue');

      return { code: patched, map: null };
    },
  };
}

function replaceExactlyOnce(code: string, search: string, replacement: string, label: string): string {
  const first = code.indexOf(search);
  if (first === -1 || code.indexOf(search, first + search.length) !== -1) {
    throw new Error(`xterm IME patch could not uniquely locate ${label}; review the patch for the installed xterm version`);
  }
  return `${code.slice(0, first)}${replacement}${code.slice(first + search.length)}`;
}
