// Copyright Elasticsearch B.V. and/or licensed to Elasticsearch B.V. under one
// or more contributor license agreements. See the NOTICE file distributed with
// this work for additional information regarding copyright
// ownership. Elasticsearch B.V. licenses this file to you under
// the Apache License, Version 2.0 (the "License"); you may
// not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

// rkyv serializable interval tree.
//
// Pasted and slightly altered version of:
//
// https://github.com/main--/rust-intervaltree
//
// Original license: MIT License
//
// Copyright (c) 2018 main()
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! rkyv serializable interval tree.

use core::cmp;
use core::fmt::Debug;
use core::iter::FromIterator;
use core::ops::Range;
use rkyv::ops::ArchivedRange;
use smallvec::SmallVec;

/// An element of an interval tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Element<K, V> {
    /// The range associated with this element.
    pub range: Range<K>,
    /// The value associated with this element.
    pub value: V,
}

impl<K, V> From<(Range<K>, V)> for Element<K, V> {
    fn from(tup: (Range<K>, V)) -> Element<K, V> {
        let (range, value) = tup;
        Element { range, value }
    }
}

#[derive(Clone, Debug, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Node<K, V> {
    element: Element<K, V>,
    max: K,
}

/// A simple and generic implementation of an immutable interval tree.
///
/// To build it, always use `FromIterator`. This is not very optimized
/// as it takes `O(log n)` stack (it uses recursion) but runs in `O(n log n)`.
#[derive(Clone, Debug, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Tree<K, V> {
    pub data: Vec<Node<K, V>>,
}

impl<K: Ord + Clone, V, I: Into<Element<K, V>>> FromIterator<I> for Tree<K, V> {
    fn from_iter<T: IntoIterator<Item = I>>(iter: T) -> Self {
        let mut nodes: Vec<_> = iter
            .into_iter()
            .map(|i| i.into())
            .map(|element| Node {
                max: element.range.end.clone(),
                element,
            })
            .collect();

        nodes.sort_unstable_by(|a, b| a.element.range.start.cmp(&b.element.range.start));

        if !nodes.is_empty() {
            Self::update_max(&mut nodes);
        }

        Tree { data: nodes }
    }
}

impl<K: Ord + Clone, V> Tree<K, V> {
    fn update_max(nodes: &mut [Node<K, V>]) -> K {
        assert!(!nodes.is_empty());
        let i = nodes.len() / 2;
        if nodes.len() > 1 {
            {
                let (left, rest) = nodes.split_at_mut(i);
                if !left.is_empty() {
                    rest[0].max = cmp::max(rest[0].max.clone(), Self::update_max(left));
                }
            }

            {
                let (rest, right) = nodes.split_at_mut(i + 1);
                if !right.is_empty() {
                    rest[i].max = cmp::max(rest[i].max.clone(), Self::update_max(right));
                }
            }
        }

        nodes[i].max.clone()
    }
}

impl<K, V> ArchivedTree<K, V>
where
    K: Ord + rkyv::Archive,
    V: rkyv::Archive,
{
    fn todo(&self) -> TodoVec {
        let mut todo = SmallVec::new();
        if !self.data.is_empty() {
            todo.push((0, self.data.len()));
        }
        todo
    }

    /// Queries the interval tree for all elements overlapping a given interval.
    ///
    /// This runs in `O(log n + m)`.
    pub fn query(&self, range: Range<K>) -> QueryIter<K, V> {
        QueryIter {
            todo: self.todo(),
            tree: self,
            query: Query::Range(range),
        }
    }

    /// Queries the interval tree for all elements containing a given point.
    ///
    /// This runs in `O(log n + m)`.
    pub fn query_point(&self, point: K) -> QueryIter<K, V> {
        QueryIter {
            todo: self.todo(),
            tree: self,
            query: Query::Point(point),
        }
    }
}

#[derive(Clone)]
enum Query<K> {
    Point(K),
    Range(Range<K>),
}

impl<K: Ord> Query<K> {
    fn point(&self) -> &K {
        match *self {
            Query::Point(ref k) => k,
            Query::Range(ref r) => &r.start,
        }
    }

    fn go_right(&self, start: &K) -> bool {
        match *self {
            Query::Point(ref k) => k >= start,
            Query::Range(ref r) => &r.end > start,
        }
    }

    fn intersect(&self, range: &ArchivedRange<K>) -> bool {
        match *self {
            Query::Point(ref k) => k < &range.end,
            Query::Range(ref r) => r.end > range.start && r.start < range.end,
        }
    }
}

type TodoVec = SmallVec<[(usize, usize); 16]>;

/// Iterator for query results.
pub struct QueryIter<'a, K: 'a + rkyv::Archive, V: 'a + rkyv::Archive> {
    tree: &'a ArchivedTree<K, V>,
    todo: TodoVec,
    query: Query<K>,
}

impl<'a, K, V> Iterator for QueryIter<'a, K, V>
where
    K: Ord + rkyv::Archive<Archived = K>,
    V: rkyv::Archive,
{
    type Item = &'a ArchivedElement<K, V>;

    fn next(&mut self) -> Option<&'a ArchivedElement<K, V>> {
        while let Some((s, l)) = self.todo.pop() {
            let i = s + l / 2;

            let node = &self.tree.data[i];
            if self.query.point() < &node.max {
                // push left
                {
                    let leftsz = i - s;
                    if leftsz > 0 {
                        self.todo.push((s, leftsz));
                    }
                }

                if self.query.go_right(&node.element.range.start) {
                    // push right
                    {
                        let rightsz = l + s - i - 1;
                        if rightsz > 0 {
                            self.todo.push((i + 1, rightsz));
                        }
                    }

                    // finally, search this
                    if self.query.intersect(&node.element.range) {
                        return Some(&node.element);
                    }
                }
            }
        }
        None
    }
}
