<script>
  // Test harness for the router under a sub-path deploy (basePath.test.ts).
  import { Router, Route, Link } from '../../router/index.js';
</script>

<Router>
  <nav>
    <Link to="/datasets/abc">dataset link</Link>
    <Link to="/sparql?query=ASK%7B%7D">sparql link</Link>
  </nav>
  <Route path="/">
    <p data-testid="page">home</p>
  </Route>
  <Route path="/datasets/:id" let:params>
    <p data-testid="page">dataset {params.id}</p>
  </Route>
  <Route path="/sparql">
    <p data-testid="page">sparql</p>
  </Route>
</Router>
