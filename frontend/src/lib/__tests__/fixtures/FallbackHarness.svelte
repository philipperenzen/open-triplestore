<!-- A router with two routes and the not-found fallback, the way App.svelte
     wires them, for notFound.test.ts. -->
<script>
  import { Router, Route, Fallback } from '../../router/index';
</script>

<Router>
  <Route path="/">home page</Route>
  <Route path="/datasets/:id" let:params>dataset {params.id}</Route>
  <Fallback>nothing here</Fallback>
</Router>
