<!-- Stands in for SparqlEditorCM: reports that it was mounted, with which
     `large` setting and how much text, without starting CodeMirror. -->
<script>
  export let query = '';
  export let large = false;
  export function flush() {}
  export function insertAtCursor() {}
</script>

<div data-testid="editor-probe" data-large={String(large)} data-len={String(query.length)}></div>
