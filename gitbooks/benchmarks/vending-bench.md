# Vending-Bench

## What It Measures

How well a memory-augmented agent makes business decisions over time. An agent manages a simulated vending machine operation over 30 days, deciding what products to stock, where to place machines, and how to price items.

## Methodology

Each method provides the agent's memory layer. The agent receives daily sales data and must make restocking and pricing decisions. Performance is measured by cumulative Profit & Loss (P\&L) over 30 simulated days.

## Methods Compared

tinycortex\_v1, mem0, scratchpad, supermemory

## Results

<div align="center"><img src="../.gitbook/assets/chart_vendingbench.png" alt="Vending-Bench P&#x26;L" width="700"></div>

| Method            | Final P\&L (Day 30) |
| ----------------- | ------------------- |
| **tinycortex\_v1** | **\~$295**          |
| scratchpad        | \~$285              |
| supermemory       | \~$215              |
| mem0              | \~$5                |

## Analysis

TinyCortex achieves the **highest cumulative P\&L by day 30** (\~$295). The interaction-weighted memory ensures the agent prioritizes learning from high-signal events (successful sales, pricing changes) while forgetting noise (random daily fluctuations). Mem0 barely breaks even, suggesting that without structured memory, the agent cannot learn from past decisions effectively.
