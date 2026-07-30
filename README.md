# Orka: The Orchestration Kernel

**Orka is a workflow orchestration engine for complex, multi-step business processes, the kind that outgrow a request-response handler but don't warrant standing up a separate workflow service.**

A process is modelled as an explicit pipeline of named steps operating on shared, typed state, with conditional branching, async-native execution, and one consistent error strategy throughout. The payoff is that the shape of a process is readable from its definition, rather than inferred from tangled call sites.

## Vision

To make complex, stateful, multi-step operations as clear to define and reason about as a single function, through a small set of primitives that hold up across languages and scale from one service to a distributed system.

## The Challenge: Managing Complex Processes

Modern applications often involve sequences of operations that are more complex than simple request-response cycles. These processes might involve:
*   Multiple distinct stages or steps.
*   Dependencies between steps.
*   Shared state that evolves as the process progresses.
*   Conditional logic that alters the execution path based on data or external events.
*   Interactions with multiple services or I/O-bound resources.
*   The need for clear error handling and potential compensation logic.

Implementing such processes directly within application code can lead to tangled logic, poor maintainability, and difficulty in understanding the overall flow.

## The Orka Solution: Principled Orchestration

Orka structures these processes around a few principles:

*   **Pipelines as First-Class Citizens:** Processes are explicitly defined as "pipelines": ordered sequences of distinct, named steps.
*   **Decoupled Step Logic:** Each step's business logic is encapsulated, promoting modularity and testability.
*   **Managed Shared State:** Pipelines operate on a well-defined shared context (or state object) that evolves through the pipeline, with mechanisms for safe concurrent access if needed.
*   **Conditional Branching:** The framework provides robust mechanisms for conditional execution paths, allowing pipelines to adapt dynamically to different inputs or states. This can involve dispatching to specialized sub-pipelines or alternative steps.
*   **Clear Control Flow:** Explicit signals (e.g., "continue," "stop") manage the progression through the pipeline.
*   **Unified Error Handling Strategy:** A consistent approach to how errors are reported, propagated, and potentially handled by the orchestrator or specific steps.
*   **Asynchronous Native:** Designed with asynchronous operations in mind, suitable for I/O-bound tasks and scalable systems.
*   **Extensibility & Pluggability:** The design encourages extending the system with custom step logic, and the conditional branching can act as a form of plugin architecture for different strategies within a workflow.

## Core Concepts (Language Agnostic)

*   **Workflow/Pipeline:** The primary unit of process definition, consisting of a sequence of steps.
*   **Step:** A distinct unit of work within a pipeline. Each step can have pre-processing, main processing, and post-processing phases.
*   **Handler/Executor:** The actual code or component that executes the logic for a step or a phase of a step.
*   **Context/State:** A data structure associated with a pipeline instance, holding the information shared and modified by its steps.
*   **Conditional Scope:** A mechanism allowing a pipeline step to choose one among several possible sub-workflows (or "scoped pipelines") to execute based on specific conditions.
*   **Extractor:** Logic that derives a focused sub-context for a scoped pipeline or a specialized handler from the main pipeline's context.
*   **Provider/Factory (for Scoped Pipelines):** A mechanism to obtain or construct a scoped pipeline, which can be pre-defined or created dynamically.
*   **Registry (Optional):** A central place to manage definitions of multiple distinct workflows, allowing them to be invoked by type or name.

## Key Benefits

*   **Improved Readability & Maintainability:** Complex processes are broken down into understandable, manageable steps.
*   **Enhanced Testability:** Individual steps and pipelines can be tested in isolation.
*   **Increased Reusability:** Common sequences of steps or entire sub-pipelines can be reused across different larger workflows.
*   **Dynamic & Flexible Execution:** Conditional logic allows workflows to adapt to various scenarios without hardcoded branching in every piece of logic.
*   **Clear State Management:** Explicit context handling makes it easier to track and reason about the state of a process.
*   **Foundation for Scalability:** Asynchronous design and clear separation of concerns lay a good foundation for building scalable systems.

## Use Cases

Orka is well-suited for a variety of applications where multi-step processes are central, such as:

*   **E-commerce:** Order processing (payment, inventory, notification, shipping), user registration, return management.
*   **Financial Services:** Loan application processing, trade execution workflows, compliance checks.
*   **Data Processing ETLs:** Multi-stage data ingestion, transformation, and loading pipelines.
*   **SaaS Applications:** User onboarding sequences, subscription lifecycle management, feature provisioning.
*   **Business Process Management (BPM):** Implementing defined business rules and operational flows.
*   **CI/CD Pipelines:** Orchestrating build, test, and deployment stages (though dedicated CI/CD tools are often more specialized).
*   **Incident Management:** Automated response workflows based on alert types.

## Implementations

*   [**Orka for Rust (`orka`)**](./core): available now.
    *   Type-safe and asynchronous. Pipelines are generic over both their context data and their error type, so step handlers integrate directly with an application's own error enum. Shared state lives in `ContextData<T>` (`Arc<RwLock<T>>`), and conditional branching dispatches to fully typed sub-pipelines operating on extracted sub-contexts.
    *   See [`core/README.md`](./core/README.md) to get started.

Implementations in other languages may occur. The [core concepts](#core-concepts-language-agnostic) above are the contract they share.
