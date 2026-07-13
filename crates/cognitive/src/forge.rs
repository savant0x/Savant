//! SIMD Genetic Forge
//!
//! This module implements a Genetic Algorithm (GA) to optimize cognitive
//! parameters (e.g., DspConfig). It is designed to be SIMD-friendly for
//! high-throughput fitness evaluation over large trajectory datasets.

use crate::predictor::{DspConfig, DspPredictor};
use rand::prelude::*;

/// A chromosome representing a potential DspConfig.
#[derive(Debug, Clone, Copy)]
pub struct ConfigChromosome {
    pub tau: f32,
    pub beta: i32,
}

impl From<ConfigChromosome> for DspConfig {
    fn from(c: ConfigChromosome) -> Self {
        DspConfig {
            tau: c.tau,
            beta: c.beta,
            max_speculative_steps: 10,
            max_history_size: 1000,
            genetic_max_generations: 100,
            genetic_convergence_threshold: 0.01,
            genetic_population_size: 50,
            genetic_mutation_rate: 0.1,
        }
    }
}

/// The Genetic Forge engine.
pub struct GeneticForge {
    pub population_size: usize,
    pub mutation_rate: f32,
    pub max_generations: usize,
    pub convergence_threshold: f32,
}

impl GeneticForge {
    pub fn new(population_size: usize, mutation_rate: f32) -> Self {
        Self {
            population_size,
            mutation_rate,
            max_generations: 100,
            convergence_threshold: 0.01,
        }
    }

    /// Creates a GeneticForge from a DspConfig, reading genetic parameters
    /// from the configuration instead of using hardcoded values.
    pub fn from_config(config: &DspConfig) -> Self {
        Self {
            population_size: config.genetic_population_size,
            mutation_rate: config.genetic_mutation_rate,
            max_generations: config.genetic_max_generations,
            convergence_threshold: config.genetic_convergence_threshold,
        }
    }

    /// Evolves the population to find the optimal DspConfig.
    ///
    /// # Arguments
    /// * `training_data` - Pairs of (complexity, actual_optimal_k)
    pub fn evolve(&self, training_data: &[(f32, u32)]) -> DspConfig {
        if self.population_size == 0 {
            tracing::warn!("GeneticForge: population_size is 0, returning default config");
            return DspConfig::default();
        }

        let mut rng = thread_rng();
        let mut population: Vec<ConfigChromosome> = (0..self.population_size)
            .map(|_| ConfigChromosome {
                tau: rng.gen_range(0.1..0.9),
                beta: rng.gen_range(-2..3),
            })
            .collect();

        let mut best_fitness = -1.0;
        let mut generations_since_improvement = 0;
        const CONVERGENCE_PLATEAU: usize = 3;

        // Perform up to max_generations of evolution with early stopping (HS-009)
        for gen in 0..self.max_generations {
            // 1. Evaluate Fitness (structured for potential SIMD autovectorization)
            let mut fitness_scores: Vec<(f32, ConfigChromosome)> = population
                .iter()
                .map(|&c| {
                    let score = self.calculate_fitness(c, training_data);
                    (score, c)
                })
                .collect();

            // 2. Selection (Sort by fitness descending: higher is better)
            fitness_scores.sort_by(|a, b| b.0.total_cmp(&a.0));

            let current_best = fitness_scores[0].0;

            // 🏰 AAA: Convergence Detection Logic
            if current_best > best_fitness + self.convergence_threshold {
                best_fitness = current_best;
                generations_since_improvement = 0;
            } else {
                generations_since_improvement += 1;
            }

            if generations_since_improvement >= CONVERGENCE_PLATEAU {
                tracing::info!(
                    "GeneticForge: Convergence target met at generation {}. Best Fitness: {:.8}",
                    gen,
                    best_fitness
                );
                break;
            }

            // 3. Breeding (Top 50% survive and reproduce)
            let survivors: Vec<ConfigChromosome> = fitness_scores
                .iter()
                .take(self.population_size / 2)
                .map(|(_, c)| *c)
                .collect();

            let mut next_gen = survivors.clone();
            while next_gen.len() < self.population_size {
                let Some(parent1) = survivors.choose(&mut rng) else {
                    break;
                };
                let Some(parent2) = survivors.choose(&mut rng) else {
                    break;
                };
                let parent1 = *parent1;
                let parent2 = *parent2;

                // Crossover & Mutation
                let mut child = ConfigChromosome {
                    tau: if rng.gen() { parent1.tau } else { parent2.tau },
                    beta: if rng.gen() {
                        parent1.beta
                    } else {
                        parent2.beta
                    },
                };

                if rng.gen::<f32>() < self.mutation_rate {
                    child.tau += rng.gen_range(-0.1..0.1);
                    child.tau = child.tau.clamp(0.1, 0.9);
                }

                next_gen.push(child);
            }
            population = next_gen;
        }

        population[0].into()
    }

    /// Calculates fitness based on inverse loss across training data.
    fn calculate_fitness(&self, chromosome: ConfigChromosome, training_data: &[(f32, u32)]) -> f32 {
        let mut predictor = match DspPredictor::new(chromosome.into()) {
            Ok(p) => p,
            Err(_) => return 0.0,
        };
        let mut total_loss = 0.0;

        for &(complexity, actual) in training_data {
            let predicted = predictor.predict_optimal_k(complexity);
            // fitness = 1 / (1 + total_loss)
            let diff = actual as f32 - predicted as f32;
            total_loss += diff.powi(2);
        }

        1.0 / (1.0 + total_loss)
    }
}
