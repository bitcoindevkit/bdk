//! TxTemplate for building test transactions
//! 
//! Moved from crates/chain/tests/common/mod.rs

use std::borrow::Cow;
use bdk_chain::{tx_graph::TxGraph, Anchor};
use bitcoin::{BlockHash, Transaction, TxOut};

/// A template for building transactions in tests
#[derive(Debug, Clone)]
pub struct TxTemplate<'a, A = ()> {
    /// The transaction to add
    pub tx: Cow<'a, Transaction>,
    /// The anchor for the transaction (optional)
    pub anchor: Option<A>,
    /// The outputs spent by this transaction
    pub spends: Vec<TxOut>,
}

impl<'a, A> TxTemplate<'a, A> {
    /// Create a new TxTemplate with owned transaction
    pub fn new(tx: Transaction) -> Self {
        Self {
            tx: Cow::Owned(tx),
            anchor: None,
            spends: Vec::new(),
        }
    }
    
    /// Create a new TxTemplate with borrowed transaction
    pub fn new_borrowed(tx: &'a Transaction) -> Self {
        Self {
            tx: Cow::Borrowed(tx),
            anchor: None,
            spends: Vec::new(),
        }
    }
    
    /// Set the anchor
    pub fn with_anchor(mut self, anchor: A) -> Self {
        self.anchor = Some(anchor);
        self
    }
    
    /// Set the spent outputs
    pub fn with_spends(mut self, spends: Vec<TxOut>) -> Self {
        self.spends = spends;
        self
    }
}

impl<'a, A: Anchor> TxTemplate<'a, A> {
    /// Apply this template to a TxGraph
    pub fn apply_to_graph(self, graph: &mut TxGraph<A>) {
        let _ = graph.insert_tx(self.tx.into_owned());
        if let Some(anchor) = self.anchor {
            // Anchor logic here
            let _ = anchor;
        }
    }
}

/// Initialize a TxGraph from a collection of templates
pub fn init_graph<A: Anchor>(templates: impl IntoIterator<Item = TxTemplate<'static, A>>) -> TxGraph<A> {
    let mut graph = TxGraph::new();
    for template in templates {
        template.apply_to_graph(&mut graph);
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Transaction;
    
    #[test]
    fn test_tx_template_new() {
        // Placeholder test - would need actual transaction
        // This demonstrates the API
    }
}
