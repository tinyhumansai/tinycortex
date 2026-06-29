# Company & Team KB Agent

{% hint style="warning" %}
This use case is a **work in progress**. We're actively building and refining it with early partners.
{% endhint %}

Every company has scattered knowledge across docs, Slack threads, Notion pages, wikis, and people's heads. A TinyCortex-powered KB agent ingests all of it and builds a memory that understands your organization, not just your documents.

## How It Works

* **Ingest everything**: Docs, meeting notes, Slack channels, internal wikis, onboarding guides all indexed into a knowledge graph that captures entities, relationships, and context.
* **Stays current automatically**: As documents are updated and new conversations happen, the knowledge graph evolves. Outdated information decays; frequently referenced knowledge stays prominent.
* **Team-aware retrieval**: The agent understands organizational structure who owns what, which team built which system, and where decisions were made. Ask "who decided to migrate to Postgres?" and get an answer, not just a document link.
* **Interaction-weighted relevance**: Knowledge that the team actively references, updates, and discusses rises in importance. That draft RFC nobody read fades; the architecture doc everyone links to stays top of mind.

## Example

A new engineer asks "how does our auth system work?" The KB agent pulls together context from the original design doc, a Slack thread where the team discussed edge cases, and the most recent PR that changed the token format synthesizing a current, accurate answer.
