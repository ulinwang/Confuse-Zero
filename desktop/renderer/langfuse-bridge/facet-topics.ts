export type FacetTopic = {
  id: string;
  name: string;
  description: string;
  count: number;
};

export type Facet = {
  topics: FacetTopic[];
  counts: { assigned: number; total: number };
};

export function orderedFacetTopics(facet: Facet) {
  return facet.topics;
}
