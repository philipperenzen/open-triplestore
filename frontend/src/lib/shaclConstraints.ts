// Catalog of SHACL constraint "cards" for the shapes editor palette.
// Each card carries a short description, a worked example, and an insertable
// Turtle template. Adapted from a SHACL sandbox.

export interface ShaclConstraintCard {
  id: string;
  label: string;
  group: 'cardinality' | 'value' | 'string' | 'logic' | 'shape' | 'advanced';
  what: string;
  example: string;
  template: string;
}

export const SHACL_CONSTRAINT_CARDS: ShaclConstraintCard[] = [
  {
    id: 'cardinality',
    label: 'Cardinality',
    group: 'cardinality',
    what: 'How many times a property may occur (sh:minCount / sh:maxCount).',
    example: 'sh:property [\n  sh:path ex:ssn ;\n  sh:minCount 1 ;\n  sh:maxCount 1 ;\n] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:minCount 1 ;\n  sh:maxCount 1 ;\n] ;',
  },
  {
    id: 'datatype',
    label: 'Datatype',
    group: 'value',
    what: 'Restrict the literal datatype of a value (sh:datatype).',
    example: 'sh:property [ sh:path ex:name ; sh:datatype xsd:string ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:datatype xsd:string ;\n] ;',
  },
  {
    id: 'numericRange',
    label: 'Numeric range',
    group: 'value',
    what: 'Bound a numeric value (sh:minInclusive / sh:maxInclusive, …Exclusive).',
    example: 'sh:property [ sh:path ex:age ; sh:minInclusive 0 ; sh:maxInclusive 120 ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:minInclusive 0 ;\n  sh:maxInclusive 100 ;\n] ;',
  },
  {
    id: 'class',
    label: 'Class',
    group: 'value',
    what: 'Value must be an instance of a class (sh:class).',
    example: 'sh:property [ sh:path ex:address ; sh:class ex:Address ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:class ex:SomeClass ;\n] ;',
  },
  {
    id: 'nodeKind',
    label: 'Node kind',
    group: 'value',
    what: 'Value must be an IRI, blank node, or literal (sh:nodeKind).',
    example: 'sh:property [ sh:path ex:homepage ; sh:nodeKind sh:IRI ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:nodeKind sh:IRI ;\n] ;',
  },
  {
    id: 'in',
    label: 'Enumeration (sh:in)',
    group: 'value',
    what: 'Value must be one of an allowed list (sh:in).',
    example: 'sh:property [ sh:path ex:status ; sh:in ( ex:Open ex:Closed ) ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:in ( ex:Value1 ex:Value2 ) ;\n] ;',
  },
  {
    id: 'hasValue',
    label: 'Required value',
    group: 'value',
    what: 'Property must include a specific value (sh:hasValue).',
    example: 'sh:property [ sh:path ex:countryCode ; sh:hasValue "NL" ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:hasValue "VALUE" ;\n] ;',
  },
  {
    id: 'pattern',
    label: 'Pattern (regex)',
    group: 'string',
    what: 'Value must match a regular expression (sh:pattern).',
    example: 'sh:property [ sh:path ex:countryCode ; sh:pattern "^[A-Z]{2}$" ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:pattern "^.+$" ;\n] ;',
  },
  {
    id: 'length',
    label: 'Length',
    group: 'string',
    what: 'Constrain string length (sh:minLength / sh:maxLength).',
    example: 'sh:property [ sh:path ex:username ; sh:minLength 5 ; sh:maxLength 30 ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:minLength 1 ;\n  sh:maxLength 30 ;\n] ;',
  },
  {
    id: 'languageIn',
    label: 'Language whitelist',
    group: 'string',
    what: 'Allowed language tags for literals (sh:languageIn).',
    example: 'sh:property [ sh:path rdfs:label ; sh:languageIn ( "en" "nl" ) ; ] ;',
    template: 'sh:property [\n  sh:path rdfs:label ;\n  sh:languageIn ( "en" "nl" ) ;\n] ;',
  },
  {
    id: 'uniqueLang',
    label: 'Unique language',
    group: 'string',
    what: 'At most one literal per language tag (sh:uniqueLang).',
    example: 'sh:property [ sh:path rdfs:label ; sh:uniqueLang true ; ] ;',
    template: 'sh:property [\n  sh:path rdfs:label ;\n  sh:uniqueLang true ;\n] ;',
  },
  {
    id: 'lessThan',
    label: 'Compare properties',
    group: 'value',
    what: 'One property must be less than another (sh:lessThan / sh:lessThanOrEquals).',
    example: 'sh:property [ sh:path ex:startDate ; sh:lessThan ex:endDate ; ] ;',
    template: 'sh:property [\n  sh:path ex:propertyA ;\n  sh:lessThan ex:propertyB ;\n] ;',
  },
  {
    id: 'equalsDisjoint',
    label: 'Equals / disjoint',
    group: 'value',
    what: 'Two properties must share (sh:equals) or never share (sh:disjoint) values.',
    example: 'sh:property [ sh:path ex:workEmail ; sh:disjoint ex:personalEmail ; ] ;',
    template: 'sh:property [\n  sh:path ex:propertyA ;\n  sh:equals ex:propertyB ;\n] ;',
  },
  {
    id: 'node',
    label: 'Nested node shape',
    group: 'shape',
    what: 'Value must conform to another node shape (sh:node).',
    example: 'sh:property [ sh:path ex:address ; sh:node ex:AddressShape ; ] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:node ex:SomeOtherShape ;\n] ;',
  },
  {
    id: 'qualifiedValue',
    label: 'Qualified value',
    group: 'shape',
    what: 'A minimum number of values must conform to a shape (sh:qualifiedValueShape).',
    example: 'sh:property [\n  sh:path ex:knows ;\n  sh:qualifiedValueShape [ sh:class ex:Person ] ;\n  sh:qualifiedMinCount 1 ;\n] ;',
    template: 'sh:property [\n  sh:path ex:someProperty ;\n  sh:qualifiedValueShape [\n    sh:class ex:SomeClass ;\n  ] ;\n  sh:qualifiedMinCount 1 ;\n] ;',
  },
  {
    id: 'closed',
    label: 'Closed shape',
    group: 'shape',
    what: 'Disallow properties not listed in the shape (sh:closed).',
    example: 'sh:closed true ;\nsh:ignoredProperties ( rdf:type ) ;',
    template: 'sh:closed true ;\nsh:ignoredProperties ( rdf:type ) ;',
  },
  {
    id: 'logicalOr',
    label: 'Logical OR',
    group: 'logic',
    what: 'At least one of the listed shapes must hold (sh:or).',
    example: 'sh:or (\n  [ sh:path ex:email ; sh:minCount 1 ]\n  [ sh:path ex:phone ; sh:minCount 1 ]\n) ;',
    template: 'sh:or (\n  [ sh:path ex:email ; sh:minCount 1 ]\n  [ sh:path ex:phone ; sh:minCount 1 ]\n) ;',
  },
  {
    id: 'logicalAnd',
    label: 'Logical AND',
    group: 'logic',
    what: 'All listed shapes must hold (sh:and).',
    example: 'sh:and (\n  [ sh:property [ sh:path ex:email ; sh:minCount 1 ] ]\n  [ sh:property [ sh:path ex:age ; sh:minInclusive 18 ] ]\n) ;',
    template: 'sh:and (\n  [ sh:property [ sh:path ex:email ; sh:minCount 1 ] ]\n  [ sh:property [ sh:path ex:age ; sh:minInclusive 18 ] ]\n) ;',
  },
  {
    id: 'logicalXone',
    label: 'Exactly one (XOR)',
    group: 'logic',
    what: 'Exactly one of the listed shapes must hold (sh:xone).',
    example: 'sh:xone (\n  [ sh:path ex:email ; sh:minCount 1 ]\n  [ sh:path ex:phone ; sh:minCount 1 ]\n) ;',
    template: 'sh:xone (\n  [ sh:path ex:email ; sh:minCount 1 ]\n  [ sh:path ex:phone ; sh:minCount 1 ]\n) ;',
  },
  {
    id: 'logicalNot',
    label: 'Logical NOT',
    group: 'logic',
    what: 'The listed shape must NOT hold (sh:not).',
    example: 'sh:not [ sh:property [ sh:path ex:deprecated ; sh:minCount 1 ] ] ;',
    template: 'sh:not [\n  sh:property [\n    sh:path ex:deprecated ;\n    sh:minCount 1 ;\n  ]\n] ;',
  },
  {
    id: 'nodeShape',
    label: 'Node shape (skeleton)',
    group: 'shape',
    what: 'A full node shape targeting a class, ready to fill in.',
    example: 'ex:PersonShape a sh:NodeShape ;\n  sh:targetClass ex:Person ;\n  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ; ] .',
    template: 'ex:NewShape a sh:NodeShape ;\n  sh:targetClass ex:SomeClass ;\n  sh:property [\n    sh:path ex:someProperty ;\n    sh:minCount 1 ;\n    sh:datatype xsd:string ;\n  ] .',
  },
  {
    id: 'sparqlConstraint',
    label: 'SPARQL constraint',
    group: 'advanced',
    what: 'Custom validation via a SPARQL SELECT that returns violations (sh:sparql).',
    example: 'sh:sparql [\n  sh:message "End date must be after start date." ;\n  sh:select """\n    SELECT $this WHERE {\n      $this ex:startDate ?s ; ex:endDate ?e .\n      FILTER (?e < ?s)\n    }""" ;\n] ;',
    template: 'sh:sparql [\n  sh:message "Describe the violation" ;\n  sh:select """\n    SELECT $this WHERE {\n      # TODO: pattern that matches invalid nodes\n    }""" ;\n] ;',
  },
  {
    id: 'ruleSparql',
    label: 'Inference rule',
    group: 'advanced',
    what: 'Derive new triples via a SPARQL CONSTRUCT rule (sh:rule / SHACL-AF).',
    example: 'sh:rule [\n  a sh:SPARQLRule ;\n  sh:construct """\n    CONSTRUCT { $this ex:area ?a }\n    WHERE { $this ex:w ?w ; ex:h ?h . BIND(?w * ?h AS ?a) }""" ;\n] ;',
    template: 'sh:rule [\n  a sh:SPARQLRule ;\n  sh:construct """\n    CONSTRUCT { $this ex:newProperty ?value }\n    WHERE {\n      # TODO: rule logic\n    }""" ;\n] ;',
  },
];

export const CONSTRAINT_GROUPS: { id: ShaclConstraintCard['group']; label: string }[] = [
  { id: 'shape', label: 'Shapes' },
  { id: 'cardinality', label: 'Cardinality' },
  { id: 'value', label: 'Value' },
  { id: 'string', label: 'String' },
  { id: 'logic', label: 'Logic' },
  { id: 'advanced', label: 'Advanced' },
];
